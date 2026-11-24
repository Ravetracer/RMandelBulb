// SPDX-License-Identifier: LGPL-2.1-or-later
//! The files of a job: the server lists them (with their hashes) and makes the scene's paths
//! absolute; a client keeps received files by hash in its cache and recreates the server's
//! paths below a job folder, so relative references between files keep working.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use rmb_core::Scene;
use rmb_core::files::FileRef;

use crate::proto::{FileEntry, sha256};

/// A scene with absolute file paths and the files it needs (server side).
pub struct Prepared {
    pub scene: Scene,
    /// Local path and entry of every file.
    pub files: Vec<(PathBuf, FileEntry)>,
}

fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(p) })
}

/// Hashes of files by path, size and modification time (large models are hashed once).
fn hash_file(p: &Path) -> Result<(u64, String), String> {
    type Hashes = HashMap<(PathBuf, u64, SystemTime), String>;
    static CACHE: Mutex<Option<Hashes>> = Mutex::new(None);
    let meta = std::fs::metadata(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let key = (p.to_path_buf(), meta.len(), meta.modified().unwrap_or(SystemTime::UNIX_EPOCH));
    if let Some(h) = CACHE.lock().ok().and_then(|c| c.as_ref().and_then(|m| m.get(&key).cloned())) {
        return Ok((meta.len(), h));
    }
    let bytes = std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let h = sha256(&bytes);
    if let Ok(mut c) = CACHE.lock() {
        c.get_or_insert_with(HashMap::new).insert(key, h.clone());
    }
    Ok((meta.len(), h))
}

/// Server: makes the scene's file paths absolute and lists the files with their hashes
/// (models with their material files and nearby images, images, the formulas' maps).
pub fn prepare(scene: &Scene) -> Result<Prepared, String> {
    let mut scene = scene.clone();
    let mut paths: Vec<(PathBuf, bool)> = Vec::new();
    rmb_core::files::visit(&mut scene, &mut |kind, name| match kind {
        FileRef::Model => {
            let p = absolute(Path::new(name.trim()));
            for f in rmb_core::files::model_files(&p) {
                paths.push((absolute(&f), false));
            }
            *name = p.to_string_lossy().into_owned();
        }
        FileRef::ModelImage(dir) => {
            // Relative names stay (the folder layout is recreated); the file goes along.
            if let Some(f) = rmb_core::mesh::find_image(&dir.join("model"), name) {
                paths.push((absolute(&f), false));
            }
        }
        FileRef::Image => {
            if let Some(f) = rmb_core::files::resolve_image(name) {
                let f = absolute(&f);
                *name = f.to_string_lossy().into_owned();
                paths.push((f, false));
            }
        }
    });
    // Maps the formulas read (Mandelbulb3D map numbers), also of the keyframes.
    let mut scenes = vec![&scene];
    if let Some(a) = &scene.animation {
        scenes.extend(a.keyframes.iter().map(|k| &k.scene));
    }
    for s in scenes {
        for n in rmb_core::Program::for_scene(s).map_numbers() {
            if let Some(f) = rmb_core::maps::find(n) {
                paths.push((absolute(&f), true));
            }
        }
    }
    paths.sort();
    paths.dedup();
    let mut files = Vec::new();
    for (p, map) in paths {
        if !p.is_file() {
            continue;
        }
        let (size, hash) = hash_file(&p)?;
        files.push((p.clone(), FileEntry { path: p.to_string_lossy().into_owned(), size, hash, map }));
    }
    Ok(Prepared { scene, files })
}

/// A server path below `root`: drive letters and separators of any system, without `..`.
pub fn mirror(root: &Path, server_path: &str) -> PathBuf {
    let mut out = root.to_path_buf();
    let unified = server_path.replace('\\', "/");
    for part in Path::new(&unified).components() {
        match part {
            Component::Normal(s) => out.push(s.to_string_lossy().replace(':', "")),
            Component::Prefix(p) => out.push(p.as_os_str().to_string_lossy().replace([':', '\\', '/', '?', '.'], "")),
            _ => {}
        }
    }
    out
}

/// Client: the cache of received files, by hash.
pub struct Cache {
    pub root: PathBuf,
}

impl Cache {
    /// The cache below `dir`, or the user's cache folder.
    pub fn new(dir: Option<PathBuf>) -> Cache {
        let root = dir.unwrap_or_else(|| rmb_core::paths::cache_dir().join("net"));
        Cache { root }
    }

    fn blob(&self, hash: &str) -> PathBuf {
        self.root.join("files").join(hash.chars().filter(|c| c.is_ascii_hexdigit()).collect::<String>())
    }

    pub fn has(&self, hash: &str) -> bool {
        self.blob(hash).is_file()
    }

    /// Stores received bytes after checking their hash.
    pub fn store(&self, hash: &str, bytes: &[u8]) -> Result<(), String> {
        if sha256(bytes) != hash {
            return Err("a file arrived damaged".into());
        }
        let p = self.blob(hash);
        std::fs::create_dir_all(p.parent().unwrap_or(&self.root)).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("part");
        std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &p)).map_err(|e| format!("{}: {e}", p.display()))
    }

    /// Folder of a job (recreated server paths, maps).
    pub fn job_dir(&self, job: u64) -> PathBuf {
        self.root.join("jobs").join(format!("{job:016x}"))
    }

    /// Folder of finished results not yet confirmed by the server.
    pub fn spool_dir(&self, job: u64) -> PathBuf {
        self.root.join("spool").join(format!("{job:016x}"))
    }

    /// Puts the job's files at their server paths below the job folder (hard links to the
    /// cache, copies where links fail); maps go into `maps/` by their names. Returns the map
    /// folder.
    pub fn materialize(&self, job: u64, files: &[FileEntry]) -> Result<PathBuf, String> {
        let dir = self.job_dir(job);
        let maps = dir.join("maps");
        std::fs::create_dir_all(&maps).map_err(|e| e.to_string())?;
        for f in files {
            let target = if f.map {
                let name = Path::new(&f.path.replace('\\', "/")).file_name().map(|n| n.to_os_string()).unwrap_or_default();
                maps.join(name)
            } else {
                mirror(&dir.join("root"), &f.path)
            };
            if target.is_file() {
                continue;
            }
            std::fs::create_dir_all(target.parent().unwrap_or(&dir)).map_err(|e| e.to_string())?;
            let blob = self.blob(&f.hash);
            if std::fs::hard_link(&blob, &target).is_err() {
                std::fs::copy(&blob, &target).map_err(|e| format!("{}: {e}", target.display()))?;
            }
        }
        Ok(maps)
    }

    /// Rewrites the scene's (absolute, server) file paths to the recreated ones.
    pub fn rewrite(&self, job: u64, scene: &mut Scene) {
        let root = self.job_dir(job).join("root");
        rmb_core::files::visit(scene, &mut |kind, name| {
            let absolute = name.starts_with('/') || name.starts_with('\\') || name.as_bytes().get(1) == Some(&b':');
            let rewrite = match kind {
                FileRef::Model | FileRef::Image => absolute,
                // Relative names find their file next to the recreated model.
                FileRef::ModelImage(_) => absolute,
            };
            if rewrite {
                *name = mirror(&root, name).to_string_lossy().into_owned();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrored_paths_stay_below_the_root() {
        let root = Path::new("/cache/job");
        assert_eq!(mirror(root, "/home/a/ship.obj"), Path::new("/cache/job/home/a/ship.obj"));
        assert_eq!(mirror(root, "C:\\Models\\ship.obj"), Path::new("/cache/job/C/Models/ship.obj"));
        assert_eq!(mirror(root, "/a/../../etc/passwd"), Path::new("/cache/job/a/etc/passwd"));
    }

    #[test]
    fn files_travel_by_hash_and_scenes_point_at_them() {
        let dir = std::env::temp_dir().join(format!("rmb-net-assets-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let img = src.join("sky.png");
        image::RgbImage::new(4, 2).save(&img).unwrap();
        let mut scene = Scene::default();
        scene.background.image = img.to_string_lossy().into_owned();
        let p = prepare(&scene).unwrap();
        assert_eq!(p.files.len(), 1);
        let cache = Cache::new(Some(dir.join("cache")));
        let (path, entry) = &p.files[0];
        assert!(!cache.has(&entry.hash));
        cache.store(&entry.hash, &std::fs::read(path).unwrap()).unwrap();
        assert!(cache.store(&entry.hash, b"other").is_err());
        cache.materialize(7, std::slice::from_ref(entry)).unwrap();
        let mut s = p.scene.clone();
        cache.rewrite(7, &mut s);
        assert!(Path::new(&s.background.image).is_file(), "{}", s.background.image);
        assert!(s.background.image.starts_with(&*cache.root.to_string_lossy()));
        std::fs::remove_dir_all(&dir).ok();
    }
}

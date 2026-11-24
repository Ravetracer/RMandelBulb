// SPDX-License-Identifier: LGPL-2.1-or-later
//! The files a scene depends on (3D models, images, Mandelbulb3D maps): found, listed and
//! renamed, so a scene can be rendered on another machine (network rendering).

use std::path::{Path, PathBuf};

use crate::scene::Scene;

/// What a file name in a scene refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRef {
    /// A 3D model file.
    Model,
    /// An image of a model's material: absolute, or relative to the model's folder (given).
    ModelImage(PathBuf),
    /// An image path, a file name in the map folders, or a Mandelbulb3D map number.
    Image,
}

/// Calls `f` for every file name in `scene` and its keyframe scenes (which may change it).
pub fn visit(scene: &mut Scene, f: &mut dyn FnMut(FileRef, &mut String)) {
    visit_one(scene, f);
    if let Some(a) = scene.animation.as_mut() {
        for k in &mut a.keyframes {
            visit_one(&mut k.scene, f);
        }
    }
}

fn visit_one(scene: &mut Scene, f: &mut dyn FnMut(FileRef, &mut String)) {
    let image = |s: &mut String, f: &mut dyn FnMut(FileRef, &mut String)| {
        if !s.trim().is_empty() {
            f(FileRef::Image, s);
        }
    };
    image(&mut scene.background.image, f);
    image(&mut scene.material.color_map.image, f);
    for l in &mut scene.material.layers {
        image(&mut l.image, f);
        image(&mut l.mask, f);
    }
    for l in &mut scene.lighting.lights {
        image(&mut l.flare.ghost_image, f);
    }
    for o in &mut scene.objects {
        for l in &mut o.material.layers {
            image(&mut l.image, f);
            image(&mut l.mask, f);
        }
        if let crate::compose::Shape::Model(m) = &mut o.shape {
            if m.path.trim().is_empty() {
                continue;
            }
            f(FileRef::Model, &mut m.path);
            let dir = Path::new(&m.path).parent().map(Path::to_path_buf).unwrap_or_default();
            for mo in &mut m.materials {
                for s in [&mut mo.bump, &mut mo.specular, &mut mo.normal].into_iter().chain(mo.texture.as_mut()) {
                    if !s.trim().is_empty() {
                        f(FileRef::ModelImage(dir.clone()), s);
                    }
                }
            }
        }
    }
}

/// The file an image name of a scene refers to: an existing path, a file in the map folders,
/// or a Mandelbulb3D map number.
pub fn resolve_image(name: &str) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if let Ok(n) = name.parse::<i32>() {
        return crate::maps::find(n);
    }
    let p = Path::new(name);
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    crate::maps::search_dirs().into_iter().map(|d| d.join(name)).find(|f| f.is_file())
}

/// The files a 3D model reads: the model, OBJ material libraries and their images, glTF
/// buffers and images, and the images near the model that materials may be matched to.
pub fn model_files(model: &Path) -> Vec<PathBuf> {
    let mut out = vec![model.to_path_buf()];
    let dir = model.parent().unwrap_or(Path::new("."));
    let ext = model.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).unwrap_or_default();
    let add = |out: &mut Vec<PathBuf>, rel: &str| {
        let rel = rel.trim().replace('\\', "/");
        if rel.is_empty() || rel.starts_with("data:") {
            return;
        }
        let p = dir.join(percent_decode(&rel));
        if p.is_file() {
            out.push(p);
        }
    };
    match ext.as_str() {
        "obj" => {
            for line in std::fs::read_to_string(model).unwrap_or_default().lines() {
                if let Some(lib) = line.trim().strip_prefix("mtllib ") {
                    let mtl = dir.join(lib.trim());
                    add(&mut out, lib);
                    // Images of the material library: the last word of map/bump lines.
                    for l in std::fs::read_to_string(&mtl).unwrap_or_default().lines() {
                        let l = l.trim();
                        let key = l.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                        if (key.starts_with("map_") || matches!(key.as_str(), "bump" | "disp" | "decal" | "norm" | "refl"))
                            && let Some(file) = l.split_whitespace().last()
                        {
                            add(&mut out, file);
                        }
                    }
                }
            }
        }
        "gltf" => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(model).unwrap_or_default()) {
                for key in ["buffers", "images"] {
                    for item in v.get(key).and_then(|b| b.as_array()).into_iter().flatten() {
                        if let Some(uri) = item.get("uri").and_then(|u| u.as_str()) {
                            add(&mut out, uri);
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out.extend(crate::mesh::nearby_images(model));
    out.sort();
    out.dedup();
    out
}

/// `%20` and the like in URIs.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_models_images_and_keyframes() {
        let mut s = Scene::default();
        s.background.image = "sky.jpg".into();
        let mut m = crate::compose::ModelRef { path: "/models/ship.obj".into(), ..Default::default() };
        m.materials.push(crate::mesh::MaterialOverride { name: "hull".into(), bump: "hull_bump.png".into(), ..Default::default() });
        s.objects.push(crate::compose::SceneObject { shape: crate::compose::Shape::Model(m), ..Default::default() });
        let mut anim = crate::animation::Animation::default();
        anim.keyframes.push(crate::animation::Keyframe { scene: s.clone(), ..Default::default() });
        s.animation = Some(anim);
        let mut seen = Vec::new();
        visit(&mut s, &mut |k, name| {
            seen.push((k, name.clone()));
            *name = format!("/x{name}");
        });
        assert_eq!(seen.len(), 6, "{seen:?}");
        // The model's folder after its own rename (relative images sit next to the new path).
        assert!(seen.contains(&(FileRef::ModelImage(PathBuf::from("/x/models")), "hull_bump.png".into())), "{seen:?}");
        assert_eq!(s.background.image, "/xsky.jpg");
        assert_eq!(s.animation.as_ref().unwrap().keyframes[0].scene.background.image, "/xsky.jpg");
        assert_eq!(percent_decode("my%20tex.png"), "my tex.png");
    }
}

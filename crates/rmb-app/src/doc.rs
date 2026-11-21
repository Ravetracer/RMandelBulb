// SPDX-License-Identifier: LGPL-2.1-or-later

//! The open document: scene, file, undo history.
//!
//! Edits change `scene` directly; `commit` (called once per frame while no pointer button is
//! held) records a new undo step when the scene differs from the last recorded one, so a slider
//! drag becomes one step.

use std::path::PathBuf;

use rmb_core::Scene;

const MAX_UNDO: usize = 200;

pub struct Document {
    pub scene: Scene,
    pub path: Option<PathBuf>,
    /// Scene as last saved (unsaved-changes marker).
    saved: Scene,
    undo: Vec<Scene>,
    redo: Vec<Scene>,
    /// Last recorded state.
    current: Scene,
}

impl Document {
    pub fn new(scene: Scene) -> Self {
        Document { saved: scene.clone(), current: scene.clone(), scene, path: None, undo: Vec::new(), redo: Vec::new() }
    }

    /// Records an undo step if the scene changed since the last one.
    pub fn commit(&mut self) {
        if self.scene != self.current {
            let prev = std::mem::replace(&mut self.current, self.scene.clone());
            self.undo.push(prev);
            if self.undo.len() > MAX_UNDO {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.scene != self.current
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo(&mut self) {
        self.commit();
        if let Some(s) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.current, s.clone()));
            self.scene = s;
        }
    }

    pub fn redo(&mut self) {
        if let Some(s) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.current, s.clone()));
            self.scene = s;
        }
    }

    /// Replaces the scene as one undoable step (e.g. a reset or an imported formula).
    pub fn replace(&mut self, scene: Scene) {
        self.commit();
        self.scene = scene;
        self.commit();
    }

    /// Loads a new document (clears the history).
    pub fn load(&mut self, scene: Scene, path: Option<PathBuf>) {
        *self = Document::new(scene);
        self.path = path;
    }

    pub fn mark_saved(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.saved = self.scene.clone();
    }

    pub fn modified(&self) -> bool {
        self.scene != self.saved
    }

    pub fn title(&self) -> String {
        let name = self.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "Untitled".into());
        if self.modified() { format!("{name} •") } else { name }
    }
}

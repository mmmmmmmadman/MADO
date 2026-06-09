//! 播放清單狀態與循環模式。
//!
//! - 拖曳多檔 → append 到 items（不是 replace）
//! - 每支影片獨立的 flip_h / flip_v（per-item，存在清單上）
//! - LoopMode 三態，由 overlay 單一按鈕循環切換
//! - advance() 在影片 EOF 時呼叫，依 LoopMode 決定下一支或停止

use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    /// 只播放單一影片，播完停在最後一幀（單支：停 / 清單：推進到下一支，全部播完停）
    Off,
    /// Loop 當下影片（單曲循環）
    One,
    /// Loop 全部播放清單
    All,
}

impl LoopMode {
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::One,
            Self::One => Self::All,
            Self::All => Self::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Loop Off",
            Self::One => "Loop One",
            Self::All => "Loop All",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Off => "→",
            Self::One => "⟳1",
            Self::All => "⟳",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PlaylistItem {
    pub path: PathBuf,
    pub flip_h: bool,
    pub flip_v: bool,
}

impl PlaylistItem {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            flip_h: false,
            flip_v: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Playlist {
    pub items: Vec<PlaylistItem>,
    pub current: usize,
    pub loop_mode: LoopMode,
}

impl Default for Playlist {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            current: 0,
            loop_mode: LoopMode::Off,
        }
    }
}

impl Playlist {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn current_item(&self) -> Option<&PlaylistItem> {
        self.items.get(self.current)
    }

    pub fn current_item_mut(&mut self) -> Option<&mut PlaylistItem> {
        self.items.get_mut(self.current)
    }

    /// 追加（拖曳多檔 / 一支一支拖 都用這個），不取代既有清單。
    /// 回傳是否清單從空變非空。
    pub fn append(&mut self, paths: Vec<PathBuf>) -> bool {
        let was_empty = self.items.is_empty();
        for p in paths {
            self.items.push(PlaylistItem::new(p));
        }
        was_empty && !self.items.is_empty()
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.current = 0;
    }

    pub fn remove(&mut self, idx: usize) {
        if idx >= self.items.len() {
            return;
        }
        self.items.remove(idx);
        if self.items.is_empty() {
            self.current = 0;
        } else if self.current >= self.items.len() {
            self.current = self.items.len() - 1;
        }
    }

    pub fn move_up(&mut self, idx: usize) -> bool {
        if idx == 0 || idx >= self.items.len() {
            return false;
        }
        self.items.swap(idx, idx - 1);
        if self.current == idx {
            self.current = idx - 1;
        } else if self.current == idx - 1 {
            self.current = idx;
        }
        true
    }

    pub fn move_down(&mut self, idx: usize) -> bool {
        if idx + 1 >= self.items.len() {
            return false;
        }
        self.items.swap(idx, idx + 1);
        if self.current == idx {
            self.current = idx + 1;
        } else if self.current == idx + 1 {
            self.current = idx;
        }
        true
    }

    pub fn select(&mut self, idx: usize) -> bool {
        if idx < self.items.len() {
            self.current = idx;
            true
        } else {
            false
        }
    }

    /// 影片播完時呼叫，回傳下一支 index（None = 應停止 / 沒有下一支）。
    pub fn advance(&mut self) -> Option<usize> {
        if self.items.is_empty() {
            return None;
        }
        match self.loop_mode {
            LoopMode::One => Some(self.current),
            LoopMode::All => {
                self.current = (self.current + 1) % self.items.len();
                Some(self.current)
            }
            LoopMode::Off => {
                if self.current + 1 < self.items.len() {
                    self.current += 1;
                    Some(self.current)
                } else {
                    None
                }
            }
        }
    }
}

/// 判斷檔案副檔名是否為 MADO 支援的影片格式（AVFoundation 原生）。
pub fn is_video_file(p: &std::path::Path) -> bool {
    let ext = match p.extension().and_then(|s| s.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return false,
    };
    matches!(
        ext.as_str(),
        "mp4" | "mov" | "m4v" | "qt" | "mkv" | "avi" | "mpg" | "mpeg" | "3gp" | "webm"
    )
}

use crate::compositor::{Component, Context, Event, EventResult};
use helix_core::Position;
use helix_view::{
    graphics::{Color, CursorKind, Rect},
    input::{KeyCode, KeyModifiers},
    Editor,
};
use tui::{
    buffer::Buffer as Surface,
    widgets::{Block, Borders, Widget},
};

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
struct TreeNode {
    path: PathBuf,
    name: String,
    is_dir: bool,
    depth: usize,
    expanded: bool,
    children: Vec<TreeNode>,
    children_loaded: bool,
}

impl TreeNode {
    fn new(path: PathBuf, depth: usize) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let is_dir = path.is_dir();
        Self {
            path,
            name,
            is_dir,
            depth,
            expanded: false,
            children: Vec::new(),
            children_loaded: false,
        }
    }

    fn load_children(&mut self) {
        if self.children_loaded || !self.is_dir {
            return;
        }
        self.children_loaded = true;

        let Ok(entries) = std::fs::read_dir(&self.path) else {
            return;
        };

        let mut children: Vec<TreeNode> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                // Skip hidden files and common ignored directories
                !name.starts_with('.')
                    && name != "node_modules"
                    && name != "target"
                    && name != "__pycache__"
            })
            .map(|e| TreeNode::new(e.path(), self.depth + 1))
            .collect();

        // Sort: directories first, then alphabetical
        children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        self.children = children;
    }

    fn toggle(&mut self) {
        if self.is_dir {
            self.expanded = !self.expanded;
            if self.expanded {
                self.load_children();
            }
        }
    }

    /// Flatten the tree into a visible list for rendering
    fn flatten(&self) -> Vec<&TreeNode> {
        let mut result = vec![self];
        if self.expanded {
            for child in &self.children {
                result.extend(child.flatten());
            }
        }
        result
    }

    /// Get the icon for this node
    fn icon(&self) -> &str {
        if self.is_dir {
            if self.expanded {
                "󰝰 "
            } else {
                "󰉋 "
            }
        } else {
            // Simple file type detection by extension
            match self
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
            {
                "rs" => "󱘗 ",
                "toml" => " ",
                "md" => " ",
                "json" => " ",
                "yaml" | "yml" => " ",
                "js" | "jsx" => "󰌞 ",
                "ts" | "tsx" => " ",
                "py" => " ",
                "go" => "󰟓 ",
                "lua" => " ",
                "sh" | "bash" | "zsh" => " ",
                "lock" => " ",
                "txt" => "󰈙 ",
                "gitignore" => " ",
                _ => "󰈔 ",
            }
        }
    }

}

pub struct TreeExplorer {
    root: TreeNode,
    cursor: usize,
    scroll_offset: usize,
    pub focused: bool,
    pub git_statuses: std::collections::HashMap<PathBuf, char>,
}

impl TreeExplorer {
    pub fn new(root_path: PathBuf) -> Self {
        let mut root = TreeNode::new(root_path, 0);
        root.expanded = true;
        root.load_children();

        let mut explorer = Self {
            root,
            cursor: 0,
            scroll_offset: 0,
            focused: true,
            git_statuses: std::collections::HashMap::new(),
        };
        explorer.refresh();
        explorer
    }

    pub fn refresh(&mut self) {
        self.git_statuses = Self::get_git_statuses(&self.root.path);
        Self::refresh_node(&mut self.root);
    }

    fn refresh_node(node: &mut TreeNode) {
        if node.is_dir && node.children_loaded {
            let Ok(entries) = std::fs::read_dir(&node.path) else {
                return;
            };
            let mut new_children: Vec<TreeNode> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    !name.starts_with('.')
                        && name != "node_modules"
                        && name != "target"
                        && name != "__pycache__"
                })
                .map(|e| {
                    let mut child = TreeNode::new(e.path(), node.depth + 1);
                    if let Some(old_child) = node.children.iter().find(|c| c.path == child.path) {
                        child.expanded = old_child.expanded;
                        child.children_loaded = old_child.children_loaded;
                        child.children = old_child.children.clone();
                    }
                    if child.expanded && !child.children_loaded {
                        child.load_children();
                    }
                    child
                })
                .collect();

            new_children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            });

            node.children = new_children;

            for child in &mut node.children {
                Self::refresh_node(child);
            }
        }
    }

    fn get_git_statuses(repo_root: &Path) -> std::collections::HashMap<PathBuf, char> {
        let mut statuses = std::collections::HashMap::new();
        let Ok(output) = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(repo_root)
            .output() else {
                return statuses;
            };

        if !output.status.success() {
            return statuses;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines() {
            if line.len() > 3 {
                let status_char = line.chars().next().unwrap_or(' ');
                let status_char2 = line.chars().nth(1).unwrap_or(' ');
                let final_char = if status_char == '?' && status_char2 == '?' {
                    '?'
                } else if status_char != ' ' {
                    status_char
                } else {
                    status_char2
                };

                let path_str = &line[3..];
                let path_str = if final_char == 'R' {
                    path_str.split(" -> ").last().unwrap_or(path_str)
                } else {
                    path_str
                };

                let path_str = path_str.trim_matches('"');
                statuses.insert(repo_root.join(path_str), final_char);
            }
        }

        // Propagate statuses to parent directories
        let mut dir_statuses = std::collections::HashMap::new();
        for (path, status) in &statuses {
            let mut parent = path.parent();
            while let Some(p) = parent {
                if p == repo_root || !p.starts_with(repo_root) {
                    break;
                }
                let current = dir_statuses.get(p).copied().unwrap_or(' ');
                if *status == '?' {
                    if current == ' ' {
                        dir_statuses.insert(p.to_path_buf(), '?');
                    }
                } else {
                    dir_statuses.insert(p.to_path_buf(), 'M');
                }
                parent = p.parent();
            }
        }
        statuses.extend(dir_statuses);
        statuses
    }

    fn visible_items(&self) -> Vec<&TreeNode> {
        self.root.flatten()
    }

    fn toggle_node_at_path(&mut self, target: &Path) {
        Self::toggle_recursive(&mut self.root, target);
    }

    fn toggle_recursive(node: &mut TreeNode, target: &Path) -> bool {
        if node.path == target {
            node.toggle();
            return true;
        }
        if node.expanded {
            for child in &mut node.children {
                if Self::toggle_recursive(child, target) {
                    return true;
                }
            }
        }
        false
    }

    fn move_cursor(&mut self, delta: i32, area_height: u16) {
        let items = self.visible_items();
        let len = items.len();
        if len == 0 {
            return;
        }

        let new_cursor = if delta > 0 {
            (self.cursor + delta as usize).min(len - 1)
        } else {
            self.cursor.saturating_sub((-delta) as usize)
        };

        self.cursor = new_cursor;

        // Keep cursor in view
        let visible_height = area_height.saturating_sub(2) as usize; // account for border
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor.saturating_sub(visible_height - 1);
        }
    }

    /// Track which children are "last" in their parent for proper tree guides
    fn get_is_last_map(node: &TreeNode) -> Vec<(PathBuf, bool)> {
        let mut result = Vec::new();
        result.push((node.path.clone(), true)); // root is always "last"

        if node.expanded {
            Self::collect_last_info(node, &mut result);
        }
        result
    }

    fn collect_last_info(parent: &TreeNode, result: &mut Vec<(PathBuf, bool)>) {
        let child_count = parent.children.len();
        for (i, child) in parent.children.iter().enumerate() {
            let is_last = i == child_count - 1;
            result.push((child.path.clone(), is_last));
            if child.expanded {
                Self::collect_last_info(child, result);
            }
        }
    }
}

impl Component for TreeExplorer {
    fn id(&self) -> Option<&'static str> {
        Some("tree-explorer")
    }

    fn handle_event(
        &mut self,
        event: &Event,
        _ctx: &mut Context,
    ) -> EventResult {
        if !self.focused {
            return EventResult::Ignored(None);
        }
        if let Event::Key(key) = event {
            match key.code {
                // Close explorer
                KeyCode::Esc => {
                    return EventResult::Consumed(Some(Box::new(|compositor, _| {
                        compositor.remove("tree-explorer");
                    })));
                }
                // Navigate
                KeyCode::Char('j') | KeyCode::Down => {
                    self.move_cursor(1, 40); // will be corrected in render
                    return EventResult::Consumed(None);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.move_cursor(-1, 40);
                    return EventResult::Consumed(None);
                }
                // Page movements
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.move_cursor(15, 40);
                    return EventResult::Consumed(None);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.move_cursor(-15, 40);
                    return EventResult::Consumed(None);
                }
                // Go to top/bottom
                KeyCode::Char('g') => {
                    self.cursor = 0;
                    self.scroll_offset = 0;
                    return EventResult::Consumed(None);
                }
                KeyCode::Char('G') => {
                    let len = self.visible_items().len();
                    if len > 0 {
                        self.cursor = len - 1;
                    }
                    return EventResult::Consumed(None);
                }
                // Toggle directory / open file
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                    let items = self.visible_items();
                    if let Some(node) = items.get(self.cursor) {
                        if node.is_dir {
                            let path = node.path.clone();
                            self.toggle_node_at_path(&path);
                        } else {
                            // Open file in editor and set explorer to unfocused
                            let path = node.path.clone();
                            let callback: crate::compositor::Callback = Box::new(
                                move |compositor: &mut crate::compositor::Compositor,
                                      ctx: &mut crate::compositor::Context| {
                                    let _ = ctx.editor.open(&path, helix_view::editor::Action::Replace);
                                    if let Some(explorer) = compositor.find_id::<TreeExplorer>("tree-explorer") {
                                        explorer.focused = false;
                                    }
                                },
                            );
                            return EventResult::Consumed(Some(callback));
                        }
                    }
                    return EventResult::Consumed(None);
                }
                // Collapse directory
                KeyCode::Char('h') | KeyCode::Left => {
                    let items = self.visible_items();
                    if let Some(node) = items.get(self.cursor) {
                        if node.is_dir && node.expanded {
                            let path = node.path.clone();
                            self.toggle_node_at_path(&path);
                        } else if node.depth > 0 {
                            // Go to parent
                            if let Some(parent) = node.path.parent() {
                                let parent = parent.to_path_buf();
                                  let items = self.visible_items();
                                for (i, item) in items.iter().enumerate() {
                                    if item.path == parent {
                                        self.cursor = i;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    return EventResult::Consumed(None);
                }
                // Create file/directory
                KeyCode::Char('a') => {
                    let items = self.visible_items();
                    if let Some(node) = items.get(self.cursor) {
                        let base_dir = if node.is_dir {
                            node.path.clone()
                        } else {
                            node.path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| self.root.path.clone())
                        };
                        let callback: crate::compositor::Callback = Box::new(
                            move |compositor: &mut crate::compositor::Compositor,
                                  _ctx: &mut crate::compositor::Context| {
                                let prompt = crate::ui::Prompt::new(
                                    "create: ".into(),
                                    None,
                                    |_, _| Vec::new(),
                                    move |compositor_ctx, input, event| {
                                        if event == crate::ui::prompt::PromptEvent::Validate && !input.is_empty() {
                                            let new_path = base_dir.join(input);
                                            let res = if input.ends_with('/') {
                                                std::fs::create_dir_all(&new_path)
                                            } else {
                                                if let Some(parent) = new_path.parent() {
                                                    let _ = std::fs::create_dir_all(parent);
                                                }
                                                std::fs::File::create(&new_path).map(|_| ())
                                            };
                                            match res {
                                                Ok(_) => {
                                                    let callback = Box::pin(async move {
                                                        let call = crate::job::Callback::EditorCompositor(Box::new(
                                                            move |_editor, compositor| {
                                                                if let Some(explorer) = compositor.find_id::<TreeExplorer>("tree-explorer") {
                                                                    explorer.refresh();
                                                                }
                                                            },
                                                        ));
                                                        Ok(call)
                                                    });
                                                    compositor_ctx.jobs.callback(callback);
                                                }
                                                Err(e) => {
                                                    compositor_ctx.editor.set_error(format!("Create failed: {}", e));
                                                }
                                            }
                                        }
                                    }
                                );
                                compositor.push(Box::new(prompt));
                            },
                        );
                        return EventResult::Consumed(Some(callback));
                    }
                }
                // Delete file/directory
                KeyCode::Char('d') => {
                    let items = self.visible_items();
                    if let Some(node) = items.get(self.cursor) {
                        let path_to_delete = node.path.clone();
                        let is_dir = node.is_dir;
                        let filename = node.name.clone();
                        let callback: crate::compositor::Callback = Box::new(
                            move |compositor: &mut crate::compositor::Compositor,
                                  _ctx: &mut crate::compositor::Context| {
                                let prompt = crate::ui::Prompt::new(
                                    format!("delete {}? (y/n): ", filename).into(),
                                    None,
                                    |_, _| Vec::new(),
                                    move |compositor_ctx, input, event| {
                                        if event == crate::ui::prompt::PromptEvent::Validate && (input == "y" || input == "yes") {
                                            let res = if is_dir {
                                                std::fs::remove_dir_all(&path_to_delete)
                                            } else {
                                                std::fs::remove_file(&path_to_delete)
                                            };
                                            match res {
                                                Ok(_) => {
                                                    let callback = Box::pin(async move {
                                                        let call = crate::job::Callback::EditorCompositor(Box::new(
                                                            move |_editor, compositor| {
                                                                if let Some(explorer) = compositor.find_id::<TreeExplorer>("tree-explorer") {
                                                                    explorer.refresh();
                                                                    let len = explorer.visible_items().len();
                                                                    if explorer.cursor >= len && len > 0 {
                                                                        explorer.cursor = len - 1;
                                                                    }
                                                                }
                                                            },
                                                        ));
                                                        Ok(call)
                                                    });
                                                    compositor_ctx.jobs.callback(callback);
                                                }
                                                Err(e) => {
                                                    compositor_ctx.editor.set_error(format!("Delete failed: {}", e));
                                                }
                                            }
                                        }
                                    }
                                );
                                compositor.push(Box::new(prompt));
                            },
                        );
                        return EventResult::Consumed(Some(callback));
                    }
                }
                // Rename file/directory
                KeyCode::Char('r') => {
                    let items = self.visible_items();
                    if let Some(node) = items.get(self.cursor) {
                        let path_to_rename = node.path.clone();
                        let filename = node.name.clone();
                        let callback: crate::compositor::Callback = Box::new(
                            move |compositor: &mut crate::compositor::Compositor,
                                  _ctx: &mut crate::compositor::Context| {
                                let prompt = crate::ui::Prompt::new(
                                    format!("rename {} to: ", filename).into(),
                                    None,
                                    |_, _| Vec::new(),
                                    move |compositor_ctx, input, event| {
                                        if event == crate::ui::prompt::PromptEvent::Validate && !input.is_empty() {
                                            if let Some(parent) = path_to_rename.parent() {
                                                let new_path = parent.join(input);
                                                match std::fs::rename(&path_to_rename, &new_path) {
                                                    Ok(_) => {
                                                        let callback = Box::pin(async move {
                                                            let call = crate::job::Callback::EditorCompositor(Box::new(
                                                                move |_editor, compositor| {
                                                                    if let Some(explorer) = compositor.find_id::<TreeExplorer>("tree-explorer") {
                                                                        explorer.refresh();
                                                                    }
                                                                },
                                                            ));
                                                            Ok(call)
                                                        });
                                                        compositor_ctx.jobs.callback(callback);
                                                    }
                                                    Err(e) => {
                                                        compositor_ctx.editor.set_error(format!("Rename failed: {}", e));
                                                    }
                                                }
                                            }
                                        }
                                    }
                                );
                                compositor.push(Box::new(prompt));
                            },
                        );
                        return EventResult::Consumed(Some(callback));
                    }
                }
                // Space + e to close (toggle)
                KeyCode::Char('q') => {
                    return EventResult::Consumed(Some(Box::new(|compositor, _| {
                        compositor.remove("tree-explorer");
                    })));
                }
                _ => {}
            }
        }
        EventResult::Consumed(None) // Consume all events when focused
    }

    fn render(&mut self, area: Rect, surface: &mut Surface, ctx: &mut Context) {
        let theme = &ctx.editor.theme;
        let bg_style = theme.get("ui.background");
        surface.set_style(area, bg_style);

        // Draw right border
        let block = Block::default()
            .borders(Borders::RIGHT)
            .style(theme.get("ui.window"));
        Widget::render(block, area, surface);

        let inner = area.clip_right(1);

        if inner.width == 0 || inner.height == 0 {
            return;
        }

        // Compute scroll offset first (before borrowing)
        let visible_height = inner.height as usize;
        let node_count = self.root.flatten().len();
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor.saturating_sub(visible_height.saturating_sub(1));
        }
        if self.scroll_offset > node_count.saturating_sub(visible_height) {
            self.scroll_offset = node_count.saturating_sub(visible_height);
        }

        let items = self.visible_items();
        let last_map: std::collections::HashMap<PathBuf, bool> =
            Self::get_is_last_map(&self.root).into_iter().collect();

        let text_style = theme.get("ui.text");
        let selected_style = theme.get("ui.menu.selected");
        let dir_style = text_style.fg(Color::Cyan);

        for (i, node) in items
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible_height)
        {
            let y = inner.y + (i - self.scroll_offset) as u16;
            let is_selected = i == self.cursor;

            let git_status = self.git_statuses.get(&node.path).copied();

            let mut style = if is_selected {
                if self.focused {
                    selected_style
                } else {
                    theme.get("ui.statusline.inactive")
                }
            } else if node.is_dir {
                dir_style
            } else {
                text_style
            };

            // Apply Git status coloring to the file/directory if not selected
            if !is_selected {
                if let Some(status) = git_status {
                    match status {
                        'M' => style = style.fg(Color::Yellow),
                        'A' => style = style.fg(Color::Green),
                        '?' => style = style.fg(Color::Indexed(8)),
                        'R' => style = style.fg(Color::Cyan),
                        _ => {}
                    }
                }
            }

            // Build the line: indent + tree guide + icon + name + git status
            let _indent_width = node.depth * 2;
            let is_last = last_map.get(&node.path).copied().unwrap_or(true);
            let guide = if node.depth == 0 {
                String::new()
            } else {
                let prefix: String = (0..node.depth.saturating_sub(1))
                    .map(|_| "│ ")
                    .collect();
                format!("{}{} ", prefix, if is_last { "╰─" } else { "├─" })
            };

            let icon = node.icon();
            let git_status_suffix = match git_status {
                Some(status) => format!(" [{}]", status),
                None => String::new(),
            };
            let display = format!("{}{}{}{}", guide, icon, node.name, git_status_suffix);

            // Fill background for selected line
            if is_selected {
                let fill_style = if self.focused {
                    selected_style
                } else {
                    theme.get("ui.statusline.inactive")
                };
                for x in inner.x..inner.x + inner.width {
                    let cell = surface.get_mut(x, y).unwrap();
                    cell.set_symbol(" ");
                    cell.set_style(fill_style);
                }
            }

            // Write the display string character by character
            let mut col = inner.x;
            for ch in display.chars() {
                if col >= inner.x + inner.width {
                    break;
                }
                let width = helix_core::unicode::width::UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
                if col + width > inner.x + inner.width {
                    break;
                }
                let cell = surface.get_mut(col, y).unwrap();
                cell.set_symbol(&ch.to_string());
                cell.set_style(style);
                col += width;
            }
        }
    }

    fn cursor(&self, _area: Rect, _ctx: &Editor) -> (Option<Position>, CursorKind) {
        (None, CursorKind::Hidden)
    }
}

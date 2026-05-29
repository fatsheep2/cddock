mod backup;
mod builds;
mod config;
mod guide;
mod http;
mod install;
mod launch;
mod nav;
mod paths;
mod platform;
mod platform_actions;

use std::{
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use config::{Config, config_path};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use image::{GenericImageView, Pixel, imageops::FilterType};
use std::collections::HashMap;

use install::{DownloadJob, DownloadPhase, ReleaseOption, fetch_release_page, start_download};
use nav::{Action, Language, Page, page_actions};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap},
};

fn main() -> io::Result<()> {
    let mut terminal = setup_terminal()?;
    let result = run_app(&mut terminal, App::default());
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, mut app: App) -> io::Result<()> {
    loop {
        app.poll_download();
        app.poll_release_fetch();
        app.refresh_game_pid();
        if app.overlay.is_none() {
            app.refresh_installed_builds();
        }
        terminal.draw(|frame| draw(frame, &app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if app.handle_key(key) => return Ok(()),
                Event::Paste(text) if app.handle_paste(&text) => return Ok(()),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

#[derive(Debug)]
struct InstalledPicker {
    title: String,
    items: Vec<String>,
    index: usize,
    builds: Vec<builds::InstalledBuild>,
}

#[derive(Debug, Clone)]
struct CachedReleasePage {
    items: Vec<ReleaseOption>,
    has_more: bool,
}

#[derive(Debug)]
struct ReleaseBrowser {
    channel: String,
    title: String,
    page: u32,
    index: usize,
    scroll_top: usize,
    has_more: bool,
    cache: HashMap<u32, CachedReleasePage>,
    loading: bool,
}

#[derive(Debug)]
struct ReleaseFetchJob {
    channel: String,
    page: u32,
    receiver: Receiver<Result<install::ReleasePage, String>>,
}

#[derive(Debug, Clone, Copy)]
enum SettingField {
    GameRoot,
    CddaPath,
    SteamShortcutName,
}

#[derive(Debug)]
struct TextInput {
    title: String,
    field: SettingField,
    value: String,
}

#[derive(Debug)]
enum Overlay {
    Installed(InstalledPicker),
    ReleaseBrowser(ReleaseBrowser),
    TextInput(TextInput),
}

#[derive(Debug)]
struct GuideSearch {
    query: String,
    build: String,
    language: String,
    language_note: Option<String>,
    results: Vec<guide::GuideSearchResult>,
    index: usize,
    scroll_top: usize,
    detail: Option<guide::GuideSearchResult>,
    detail_scroll: u16,
    detail_links: Vec<String>,
    detail_link_index: usize,
    detail_history: Vec<guide::GuideSearchResult>,
}

#[derive(Debug)]
struct App {
    config: Config,
    config_path: PathBuf,
    language: Language,
    focus: Focus,
    page_index: usize,
    action_index: usize,
    message: String,
    overlay: Option<Overlay>,
    guide_search: Option<GuideSearch>,
    guide_dataset: Option<(String, String, guide::GuideDataset)>,
    download: Option<DownloadJob>,
    release_fetch: Option<ReleaseFetchJob>,
    game_pid: Option<u32>,
    pending_active_build: Option<String>,
    installed_builds: Vec<builds::InstalledBuild>,
    builds_dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Pages,
    Actions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FocusPoint {
    focus: Focus,
    index: usize,
    x: i32,
    y: i32,
}

impl Default for App {
    fn default() -> Self {
        let config_path = config_path();
        let config = Config::load(&config_path);
        let language = config
            .language
            .as_deref()
            .and_then(Language::from_config_value)
            .unwrap_or_else(Language::detect);
        let game_root = config.game_root_path();
        let channel = config.release_channel.clone();
        let _ = paths::ensure_layout(&game_root, &channel);
        if paths::consolidate_userdata(&game_root).unwrap_or(false) {
            let _ = config.save(&config_path);
        }

        Self {
            config,
            config_path,
            language,
            focus: Focus::Pages,
            page_index: 0,
            action_index: 0,
            message: language
                .text(
                    "Ready. Arrow keys move by screen position; Enter activates.",
                    "已就绪。方向键按屏幕相对位置移动，Enter 执行。",
                )
                .to_string(),
            overlay: None,
            guide_search: None,
            guide_dataset: None,
            download: None,
            release_fetch: None,
            game_pid: None,
            pending_active_build: None,
            installed_builds: Vec::new(),
            builds_dirty: true,
        }
    }
}

impl App {
    fn page(&self) -> Page {
        Page::ALL[self.page_index]
    }

    fn actions(&self) -> &'static [Action] {
        page_actions(self.page())
    }

    fn game_root(&self) -> PathBuf {
        self.config.game_root_path()
    }

    fn poll_download(&mut self) {
        let Some(job) = self.download.as_mut() else {
            return;
        };
        job.poll();
        let phase = job.phase();
        match &phase {
            DownloadPhase::Downloading { received, total } => {
                let total_text = total
                    .map(|value| format!("{:.1} MB", value as f64 / 1_048_576.0))
                    .unwrap_or_else(|| "?".to_string());
                self.message = format!(
                    "{} {:.1} / {} MB",
                    self.language.text("Downloading", "下载中"),
                    *received as f64 / 1_048_576.0,
                    total_text
                );
            }
            DownloadPhase::Extracting => {
                self.message = self
                    .language
                    .text("Extracting build...", "正在解压版本...")
                    .to_string();
            }
            DownloadPhase::Done => {
                if let Some(build_id) = self.pending_active_build.take() {
                    let channel = self.config.channel_for_build(&build_id);
                    self.config.register_build_channel(&build_id, &channel);
                    self.config.active_build = build_id.clone();
                    self.config.release_channel = channel;
                    self.message = self.save_config_message(format!(
                        "{}: {}",
                        self.language
                            .text("Install finished, active build", "安装完成，当前版本"),
                        build_id
                    ));
                } else {
                    self.message = self
                        .language
                        .text("Install finished.", "安装完成。")
                        .to_string();
                }
                self.builds_dirty = true;
                let _ = paths::consolidate_userdata(&self.game_root());
                self.download = None;
            }
            DownloadPhase::Failed(error) => {
                self.message = format!(
                    "{}: {error}",
                    self.language.text("Install failed", "安装失败")
                );
                self.download = None;
            }
        }
    }

    fn poll_release_fetch(&mut self) {
        let Some(job) = self.release_fetch.as_ref() else {
            return;
        };
        let Ok(result) = job.receiver.try_recv() else {
            return;
        };

        let Some(job) = self.release_fetch.take() else {
            return;
        };
        match result {
            Ok(page) => self.finish_release_fetch(job.channel, job.page, page),
            Err(error) => {
                if let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut()
                    && browser.channel == job.channel
                {
                    browser.loading = false;
                }
                self.message = error;
            }
        }
    }

    fn finish_release_fetch(
        &mut self,
        channel: String,
        requested_page: u32,
        page: install::ReleasePage,
    ) {
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut() else {
            return;
        };
        if browser.channel != channel {
            return;
        }

        browser.loading = false;
        browser.page = page.page;
        browser.has_more = page.has_more;
        browser.index = 0;
        browser.scroll_top = 0;
        browser.cache.insert(
            page.page,
            CachedReleasePage {
                items: page.items,
                has_more: page.has_more,
            },
        );

        if browser
            .cache
            .get(&browser.page)
            .is_some_and(|cached| cached.items.is_empty())
        {
            if browser.has_more {
                self.message = self
                    .language
                    .text(
                        "This page had no compatible builds. Press ] for older releases.",
                        "本页没有兼容版本，按 ] 继续翻页。",
                    )
                    .to_string();
            } else {
                self.message = self
                    .language
                    .text("No compatible releases were found.", "没有找到兼容版本。")
                    .to_string();
            }
        } else {
            self.message = format!(
                "{} {} ({} {})",
                self.language.text("Page", "第"),
                requested_page,
                browser
                    .cache
                    .get(&browser.page)
                    .map(|cached| cached.items.len())
                    .unwrap_or(0),
                self.language.text("builds", "个版本"),
            );
        }
    }

    fn start_release_fetch(&mut self, channel: String, page: u32) {
        if self.release_fetch.is_some() {
            self.message = self
                .language
                .text("Release list is already loading.", "发布列表正在加载。")
                .to_string();
            return;
        }

        let (sender, receiver) = mpsc::channel();
        let channel_for_thread = channel.clone();
        thread::spawn(move || {
            let result = fetch_release_page(&channel_for_thread, page);
            let _ = sender.send(result);
        });
        self.release_fetch = Some(ReleaseFetchJob {
            channel,
            page,
            receiver,
        });
    }

    fn refresh_game_pid(&mut self) {
        if let Some(pid) = self.game_pid
            && !launch::is_process_alive(pid)
        {
            self.game_pid = None;
        }
    }

    fn refresh_installed_builds(&mut self) {
        if !self.builds_dirty {
            return;
        }
        self.installed_builds = builds::scan_installed(&self.game_root()).unwrap_or_default();
        self.builds_dirty = false;
    }

    fn handle_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }

        if self.overlay.is_some() {
            return self.handle_overlay_key(key);
        }

        if self.page() == Page::Guide && self.guide_search.is_some() {
            return self.handle_guide_key(key);
        }

        if self.download.is_some() {
            if matches!(key.code, KeyCode::Esc) {
                if let Some(job) = &self.download {
                    job.cancel();
                }
                self.message = self
                    .language
                    .text("Cancelling download...", "正在取消下载...")
                    .to_string();
            }
            return false;
        }

        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Tab | KeyCode::BackTab => self.toggle_focus(),
            KeyCode::Char('h') | KeyCode::Left => self.move_focus(FocusDirection::Left),
            KeyCode::Char('l') | KeyCode::Right => self.move_focus(FocusDirection::Right),
            KeyCode::Char('k') | KeyCode::Up => self.move_focus(FocusDirection::Up),
            KeyCode::Char('j') | KeyCode::Down => self.move_focus(FocusDirection::Down),
            KeyCode::Esc => {
                if self.page() == Page::Home {
                    return true;
                } else if self.focus == Focus::Actions {
                    self.focus_pages();
                } else {
                    self.open_page(Page::Home);
                    self.message = self
                        .language
                        .text("Returned to Home.", "已返回首页。")
                        .to_string();
                }
            }
            KeyCode::Enter => {
                if self.focus == Focus::Pages {
                    self.focus_actions();
                } else if self.actions().get(self.action_index) == Some(&Action::QuitCddock) {
                    return true;
                } else {
                    self.activate();
                }
            }
            _ => {}
        }

        false
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        match &mut self.overlay {
            Some(Overlay::Installed(picker)) => match key.code {
                KeyCode::Esc => self.close_overlay(),
                KeyCode::Char('k') | KeyCode::Up => {
                    picker.index = picker
                        .index
                        .checked_sub(1)
                        .unwrap_or(picker.items.len().saturating_sub(1));
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if !picker.items.is_empty() {
                        picker.index = (picker.index + 1) % picker.items.len();
                    }
                }
                KeyCode::Enter => self.confirm_installed_picker(),
                _ => {}
            },
            Some(Overlay::ReleaseBrowser(_browser)) => match key.code {
                KeyCode::Esc => self.close_overlay(),
                KeyCode::Char('k') | KeyCode::Up => self.browser_move_up(),
                KeyCode::Char('j') | KeyCode::Down => self.browser_move_down(),
                KeyCode::Char('h') | KeyCode::Left | KeyCode::PageUp | KeyCode::Char('[') => {
                    self.browser_prev_page();
                }
                KeyCode::Char('l')
                | KeyCode::Right
                | KeyCode::PageDown
                | KeyCode::Char(']')
                | KeyCode::Char('n') => {
                    self.browser_next_page();
                }
                KeyCode::Enter => self.confirm_release_browser(),
                _ => {}
            },
            Some(Overlay::TextInput(input)) => match key.code {
                KeyCode::Esc => self.close_overlay(),
                KeyCode::Enter => self.confirm_text_input(),
                KeyCode::Backspace => {
                    input.value.pop();
                }
                KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.value.push(ch);
                }
                _ => {}
            },
            None => {}
        }
        false
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        if let Some(Overlay::TextInput(input)) = self.overlay.as_mut() {
            input
                .value
                .extend(text.chars().filter(|ch| !ch.is_control()));
            return false;
        }

        if self.page() != Page::Guide {
            return false;
        }
        let Some(search) = self.guide_search.as_mut() else {
            return false;
        };
        if search.detail.is_some() {
            return false;
        }
        for ch in text.chars().filter(|ch| !ch.is_control()) {
            search.query.push(ch);
        }
        search.results.clear();
        search.index = 0;
        search.scroll_top = 0;
        false
    }

    fn handle_guide_key(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }

        match key.code {
            KeyCode::Esc => {
                if self
                    .guide_search
                    .as_ref()
                    .is_some_and(|search| search.detail.is_some())
                {
                    self.close_guide_detail();
                } else {
                    self.guide_search = None;
                    self.focus_actions();
                }
            }
            KeyCode::Backspace => {
                if let Some(search) = self.guide_search.as_mut() {
                    search.query.pop();
                    search.results.clear();
                    search.index = 0;
                    search.scroll_top = 0;
                    search.detail_links.clear();
                    search.detail_link_index = 0;
                    search.detail_history.clear();
                }
            }
            KeyCode::Char('q') => return true,
            KeyCode::Char('k') | KeyCode::Up => self.guide_move_up(),
            KeyCode::Char('j') | KeyCode::Down => self.guide_move_down(),
            KeyCode::Tab => self.guide_next_link(),
            KeyCode::BackTab => self.guide_previous_link(),
            KeyCode::Enter => self.confirm_guide_search(),
            KeyCode::Char(ch) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    if let Some(search) = self.guide_search.as_mut()
                        && search.detail.is_none()
                    {
                        search.query.push(ch);
                        search.results.clear();
                        search.index = 0;
                        search.scroll_top = 0;
                        search.detail_links.clear();
                        search.detail_link_index = 0;
                        search.detail_history.clear();
                    }
                }
            }
            _ => {}
        }
        false
    }

    fn close_overlay(&mut self) {
        self.overlay = None;
        self.message = self
            .language
            .text("Selection cancelled.", "已取消选择。")
            .to_string();
    }

    fn open_text_input(&mut self, field: SettingField) {
        let (title, value) = match field {
            SettingField::GameRoot => (
                self.language.text("Edit game root", "编辑游戏根目录"),
                self.config.game_root.clone(),
            ),
            SettingField::CddaPath => (
                self.language.text("Edit CDDA path", "编辑 CDDA 路径"),
                self.config.cdda_path.clone(),
            ),
            SettingField::SteamShortcutName => (
                self.language
                    .text("Edit Steam shortcut name", "编辑 Steam 快捷方式名称"),
                self.config.steam_shortcut_name.clone(),
            ),
        };
        self.overlay = Some(Overlay::TextInput(TextInput {
            title: title.to_string(),
            field,
            value,
        }));
        self.message = self
            .language
            .text(
                "Edit value, Enter saves, Esc cancels.",
                "编辑内容，Enter 保存，Esc 取消。",
            )
            .to_string();
    }

    fn confirm_text_input(&mut self) {
        let Some(Overlay::TextInput(input)) = self.overlay.take() else {
            return;
        };
        let value = input.value.trim().to_string();
        if value.is_empty() {
            self.message = self
                .language
                .text("Value cannot be empty.", "内容不能为空。")
                .to_string();
            return;
        }

        match input.field {
            SettingField::GameRoot => {
                self.config.game_root = value;
                let _ = paths::ensure_layout(&self.game_root(), &self.config.release_channel);
                self.builds_dirty = true;
            }
            SettingField::CddaPath => {
                self.config.cdda_path = value;
            }
            SettingField::SteamShortcutName => {
                self.config.steam_shortcut_name = value;
            }
        }

        self.message = self.save_config_message(
            self.language
                .text("Setting updated", "设置已更新")
                .to_string(),
        );
    }

    fn confirm_installed_picker(&mut self) {
        let Some(Overlay::Installed(picker)) = self.overlay.take() else {
            return;
        };
        let Some(build) = picker.builds.get(picker.index) else {
            self.message = self
                .language
                .text("No build selected.", "未选择版本。")
                .to_string();
            return;
        };
        self.config.active_build = build.id.clone();
        self.message = self.save_config_message(format!(
            "{}: {}",
            self.language.text("Active build", "当前版本"),
            build.id
        ));
    }

    fn confirm_release_browser(&mut self) {
        if let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_ref()
            && browser.loading
        {
            self.message = self
                .language
                .text("Release list is still loading.", "发布列表仍在加载。")
                .to_string();
            return;
        }
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.take() else {
            return;
        };
        let Some(cached) = browser.cache.get(&browser.page) else {
            self.overlay = Some(Overlay::ReleaseBrowser(browser));
            return;
        };
        let Some(release) = cached.items.get(browser.index).cloned() else {
            self.message = self
                .language
                .text("No release selected.", "未选择发布版本。")
                .to_string();
            return;
        };
        self.start_release_download(release);
    }

    fn enter_game(&mut self) -> String {
        let Some((path, userdata)) = self.active_build_and_userdata() else {
            return self
                .language
                .text(
                    "No active build selected. Choose one under Versions.",
                    "未选择当前版本，请先在版本页选择。",
                )
                .to_string();
        };

        if builds::find_executable(&path).is_none() {
            return format!(
                "{}: {}",
                self.language.text(
                    "Active build is incomplete, reinstall it",
                    "当前版本安装不完整，请重新安装"
                ),
                path.display()
            );
        }

        let tracked = self.game_pid.take();
        let stopped = launch::stop_cdda_instances(&self.game_root(), tracked).unwrap_or(0);

        let world = builds::find_most_recent_world(&userdata);
        match launch::launch_build(&path, &userdata, world.as_deref()) {
            Ok(pid) => {
                self.game_pid = Some(pid);
                let mut parts =
                    vec![self
                    .language
                    .text(
                        "Closing any running CDDA instance, then launching the active build",
                        "将关闭运行中的 CDDA 进程，并启动当前启用版本",
                    )
                    .to_string()];
                if stopped > 0 {
                    parts.push(format!(
                        "({})",
                        self.language
                            .text("stopped previous instance", "已关闭先前进程")
                    ));
                }
                if let Some(ref world_name) = world {
                    parts.push(format!(
                        "{}: {world_name}",
                        self.language.text("Resuming last world", "继续上次世界")
                    ));
                }
                parts.push(format!("pid {pid}"));
                parts.join(" · ")
            }
            Err(error) => error,
        }
    }

    fn active_build_and_userdata(&self) -> Option<(PathBuf, PathBuf)> {
        let path = builds::active_build_path(&self.game_root(), &self.config.active_build)?;
        let userdata = paths::shared_userdata_dir(&self.game_root());
        Some((path, userdata))
    }

    fn backup_saves(&self) -> String {
        let channel = if self.config.active_build.is_empty() {
            self.config.release_channel.clone()
        } else {
            self.config.channel_for_build(&self.config.active_build)
        };
        match backup::backup_saves(&self.game_root(), &channel) {
            Ok(path) => format!(
                "{}: {path}",
                self.language.text("Save backup created", "存档备份已创建")
            ),
            Err(error) => format!(
                "{}: {error}",
                self.language.text("Save backup failed", "存档备份失败")
            ),
        }
    }

    fn open_guide_search(&mut self) {
        let lang = guide::guide_language(self.language.config_value()).to_string();
        match guide::resolve_build(
            &self.game_root(),
            &self.config.active_build,
            &self.config.release_channel,
        ) {
            Ok(build) => {
                self.guide_search = Some(GuideSearch {
                    query: String::new(),
                    build,
                    language: lang,
                    language_note: None,
                    results: Vec::new(),
                    index: 0,
                    scroll_top: 0,
                    detail: None,
                    detail_scroll: 0,
                    detail_links: Vec::new(),
                    detail_link_index: 0,
                    detail_history: Vec::new(),
                });
                self.focus = Focus::Actions;
                self.message = self
                    .language
                    .text(
                        "Type a guide query, Enter searches, Enter again opens detail.",
                        "输入图鉴关键词，Enter 搜索，再按 Enter 打开详情。",
                    )
                    .to_string();
            }
            Err(error) => self.message = error,
        }
    }

    fn confirm_guide_search(&mut self) {
        let Some(search) = self.guide_search.as_ref() else {
            return;
        };
        if search.detail.is_some() {
            self.open_selected_guide_link();
            return;
        }
        let selected = search.results.get(search.index).cloned();
        let query = search.query.clone();
        let build = search.build.clone();
        let language = search.language.clone();
        if let Some(mut result) = selected {
            self.set_guide_detail(&mut result, false);
            return;
        }

        let cache_key_matches =
            self.guide_dataset
                .as_ref()
                .is_some_and(|(cached_build, cached_language, _)| {
                    cached_build == &build && cached_language == &language
                });
        if !cache_key_matches {
            match guide::load_dataset(&self.game_root(), &build, &language) {
                Ok(dataset) => {
                    if let Some(search) = self.guide_search.as_mut() {
                        search.language_note = dataset.warning().map(str::to_string);
                    }
                    self.guide_dataset = Some((build.clone(), language.clone(), dataset));
                }
                Err(error) => {
                    self.message = error;
                    return;
                }
            }
        }
        let results = self
            .guide_dataset
            .as_ref()
            .map(|(_, _, dataset)| guide::search_dataset(dataset, &query, 80))
            .unwrap_or_default();
        let count = results.len();
        if let Some(search) = self.guide_search.as_mut() {
            search.results = results;
            search.index = 0;
            search.scroll_top = 0;
            search.detail_scroll = 0;
            search.detail_links.clear();
            search.detail_link_index = 0;
            search.detail_history.clear();
        }
        let dataset_status = self
            .guide_dataset
            .as_ref()
            .map(|(_, _, dataset)| {
                let lang = dataset.language();
                let total = dataset.len();
                dataset
                    .warning()
                    .map(|warning| format!("{warning} "))
                    .unwrap_or_default()
                    + &format!("lang {lang}, indexed {total}")
            })
            .unwrap_or_default();
        let empty_hint = if count == 0 {
            self.language.text(
                "No matches; try an English item id/name or a field such as flags, material, recipe, tileset.",
                "没有匹配；可试英文物品 id/名称，或 flags、material、recipe、tileset 等字段。",
            )
        } else {
            ""
        };
        self.message = if empty_hint.is_empty() {
            format!(
                "{}: {count}. {dataset_status}",
                self.language.text("Guide results", "图鉴结果")
            )
        } else {
            format!(
                "{}: {count}. {dataset_status} {empty_hint}",
                self.language.text("Guide results", "图鉴结果")
            )
        };
    }

    fn set_guide_detail(&mut self, result: &mut guide::GuideSearchResult, push_history: bool) {
        guide::add_local_tile_info(&self.game_root(), &self.config.active_build, result);
        let links = self
            .guide_dataset
            .as_ref()
            .map(|(_, _, dataset)| {
                guide::field_target_ids(result)
                    .into_iter()
                    .filter(|id| dataset.contains_id(id))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if push_history && let Some(current) = search.detail.take() {
            search.detail_history.push(current);
        }
        search.detail = Some(result.clone());
        search.detail_scroll = 0;
        search.detail_links = links;
        search.detail_link_index = 0;
    }

    fn open_selected_guide_link(&mut self) {
        let Some(link_id) = self
            .guide_search
            .as_ref()
            .and_then(|search| search.detail_links.get(search.detail_link_index).cloned())
        else {
            self.message = self
                .language
                .text(
                    "No guide relation link is available.",
                    "当前详情没有可跳转关系。",
                )
                .to_string();
            return;
        };
        let Some(mut result) = self
            .guide_dataset
            .as_ref()
            .and_then(|(_, _, dataset)| dataset.get(&link_id))
        else {
            self.message = format!(
                "{}: {link_id}",
                self.language
                    .text("Guide relation target missing", "图鉴关系目标不存在")
            );
            return;
        };
        self.set_guide_detail(&mut result, true);
    }

    fn close_guide_detail(&mut self) {
        let links = if let Some(search) = self.guide_search.as_mut() {
            if let Some(previous) = search.detail_history.pop() {
                let links = self
                    .guide_dataset
                    .as_ref()
                    .map(|(_, _, dataset)| {
                        guide::field_target_ids(&previous)
                            .into_iter()
                            .filter(|id| dataset.contains_id(id))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                search.detail = Some(previous);
                search.detail_scroll = 0;
                Some(links)
            } else {
                search.detail = None;
                search.detail_scroll = 0;
                search.detail_links.clear();
                search.detail_link_index = 0;
                None
            }
        } else {
            None
        };
        if let Some(links) = links
            && let Some(search) = self.guide_search.as_mut()
        {
            search.detail_links = links;
            search.detail_link_index = 0;
        }
    }

    fn guide_move_up(&mut self) {
        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if search.detail.is_some() {
            search.detail_scroll = search.detail_scroll.saturating_sub(1);
            return;
        }
        if search.results.is_empty() {
            return;
        }
        search.index = search
            .index
            .checked_sub(1)
            .unwrap_or(search.results.len().saturating_sub(1));
        if search.index < search.scroll_top {
            search.scroll_top = search.index;
        }
    }

    fn guide_move_down(&mut self) {
        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if search.detail.is_some() {
            search.detail_scroll = search.detail_scroll.saturating_add(1);
            return;
        }
        if search.results.is_empty() {
            return;
        }
        search.index = (search.index + 1) % search.results.len();
        const VIEWPORT: usize = 18;
        if search.index + 1 >= search.scroll_top + VIEWPORT {
            search.scroll_top = search.index.saturating_sub(VIEWPORT - 1);
        }
    }

    fn guide_next_link(&mut self) {
        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if search.detail.is_none() || search.detail_links.is_empty() {
            return;
        }
        search.detail_link_index = (search.detail_link_index + 1) % search.detail_links.len();
    }

    fn guide_previous_link(&mut self) {
        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if search.detail.is_none() || search.detail_links.is_empty() {
            return;
        }
        search.detail_link_index = search
            .detail_link_index
            .checked_sub(1)
            .unwrap_or(search.detail_links.len() - 1);
    }

    fn browser_items(&self) -> Option<&[ReleaseOption]> {
        match self.overlay.as_ref()? {
            Overlay::ReleaseBrowser(browser) => browser
                .cache
                .get(&browser.page)
                .map(|cached| cached.items.as_slice()),
            Overlay::Installed(_) | Overlay::TextInput(_) => None,
        }
    }

    fn browser_move_up(&mut self) {
        let len = self.browser_items().map(|items| items.len()).unwrap_or(0);
        if len == 0 {
            return;
        }
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut() else {
            return;
        };
        browser.index = browser.index.checked_sub(1).unwrap_or(len - 1);
        if browser.index < browser.scroll_top {
            browser.scroll_top = browser.index;
        }
    }

    fn browser_move_down(&mut self) {
        let len = self.browser_items().map(|items| items.len()).unwrap_or(0);
        if len == 0 {
            return;
        }
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut() else {
            return;
        };
        browser.index = (browser.index + 1) % len;
        const VIEWPORT: usize = 22;
        if browser.index + 1 >= browser.scroll_top + VIEWPORT {
            browser.scroll_top = browser.index.saturating_sub(VIEWPORT - 1);
        }
    }

    fn browser_prev_page(&mut self) {
        let current = match &self.overlay {
            Some(Overlay::ReleaseBrowser(browser)) => browser.page,
            _ => return,
        };
        if current <= 1 {
            return;
        }
        self.browser_load_page(current - 1);
    }

    fn browser_next_page(&mut self) {
        let (has_more, current) = match &self.overlay {
            Some(Overlay::ReleaseBrowser(browser)) => (browser.has_more, browser.page),
            _ => return,
        };
        if !has_more {
            self.message = self
                .language
                .text(
                    "Already on the oldest fetched page.",
                    "已经是最早获取的一页。",
                )
                .to_string();
            return;
        }
        self.browser_load_page(current + 1);
    }

    fn browser_load_page(&mut self, page: u32) {
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut() else {
            return;
        };
        if browser.cache.contains_key(&page) {
            browser.page = page;
            browser.index = 0;
            browser.scroll_top = 0;
            browser.has_more = browser
                .cache
                .get(&page)
                .map(|cached| cached.has_more)
                .unwrap_or(false);
            return;
        }

        let channel = browser.channel.clone();
        browser.loading = true;
        self.message = self
            .language
            .text("Fetching release list...", "正在获取发布列表...")
            .to_string();
        self.start_release_fetch(channel, page);
    }

    fn open_installed_picker(&mut self) {
        self.refresh_installed_builds();
        let builds: Vec<_> = self
            .installed_builds
            .iter()
            .filter(|build| build.has_executable)
            .cloned()
            .collect();
        if builds.is_empty() {
            self.message = self
                .language
                .text(
                    "No launchable builds found. Install one first.",
                    "未找到可启动版本，请先安装。",
                )
                .to_string();
            return;
        }

        let items = self.iter_installed_build_items(&builds).collect();

        self.overlay = Some(Overlay::Installed(InstalledPicker {
            title: self
                .language
                .text("Select active build", "选择当前版本")
                .to_string(),
            items,
            index: 0,
            builds,
        }));
    }

    fn iter_installed_build_items<'a>(
        &'a self,
        builds: &'a [builds::InstalledBuild],
    ) -> impl Iterator<Item = String> + 'a {
        builds.iter().map(|build| {
            let marker = if build.id == self.config.active_build {
                "*"
            } else {
                " "
            };
            let exe = self.language.text("ready", "可启动");
            format!("{marker} {} [{exe}]", build.id)
        })
    }

    fn open_release_picker(&mut self, channel: &str) {
        self.message = self
            .language
            .text("Fetching release list...", "正在获取发布列表...")
            .to_string();

        let title = if channel == "stable" {
            self.language
                .text("Stable releases", "稳定版列表")
                .to_string()
        } else {
            self.language
                .text("Experimental releases", "实验版列表")
                .to_string()
        };
        self.overlay = Some(Overlay::ReleaseBrowser(ReleaseBrowser {
            channel: channel.to_string(),
            title,
            page: 1,
            index: 0,
            scroll_top: 0,
            has_more: false,
            cache: HashMap::new(),
            loading: true,
        }));
        self.start_release_fetch(channel.to_string(), 1);
    }

    fn start_release_download(&mut self, release: ReleaseOption) {
        if self.download.is_some() {
            self.message = self
                .language
                .text("A download is already running.", "已有下载任务在进行。")
                .to_string();
            return;
        }

        match start_download(&self.game_root(), release.clone()) {
            Ok(job) => {
                self.config
                    .register_build_channel(&release.build_id, &release.channel);
                self.pending_active_build = Some(release.build_id.clone());
                self.download = Some(job);
                self.message = format!(
                    "{} {}",
                    self.language.text("Downloading", "下载中"),
                    release.build_id
                );
            }
            Err(error) => self.message = error,
        }
    }

    fn move_focus(&mut self, direction: FocusDirection) {
        let current = self.current_focus_point();
        let points = self.focus_points();
        let Some(next) = spatial_focus_target(current, &points, direction) else {
            return;
        };
        self.apply_focus_point(next);
    }

    fn current_focus_point(&self) -> FocusPoint {
        let index = match self.focus {
            Focus::Pages => self.page_index,
            Focus::Actions => self.action_index,
        };
        focus_point(self.focus, index, self.page(), self.actions().len())
    }

    fn focus_points(&self) -> Vec<FocusPoint> {
        let page_points = Page::ALL
            .iter()
            .enumerate()
            .map(|(index, _)| focus_point(Focus::Pages, index, self.page(), self.actions().len()));
        let action_points = self.actions().iter().enumerate().map(|(index, _)| {
            focus_point(Focus::Actions, index, self.page(), self.actions().len())
        });
        page_points.chain(action_points).collect()
    }

    fn apply_focus_point(&mut self, point: FocusPoint) {
        match point.focus {
            Focus::Pages => {
                if self.page_index != point.index {
                    self.page_index = point.index.min(Page::ALL.len().saturating_sub(1));
                    self.action_index = 0;
                    self.on_page_changed();
                }
                self.focus = Focus::Pages;
            }
            Focus::Actions => {
                let action_count = self.actions().len();
                if action_count == 0 {
                    self.action_index = 0;
                } else {
                    self.action_index = point.index.min(action_count - 1);
                }
                self.focus = Focus::Actions;
            }
        }
    }

    fn on_page_changed(&mut self) {
        if self.page() == Page::Builds {
            self.builds_dirty = true;
        }
        self.message = format!(
            "{} {}.",
            self.language.text("Opened", "已打开"),
            self.page().title(self.language)
        );
    }

    fn toggle_focus(&mut self) {
        match self.focus {
            Focus::Pages => self.focus_actions(),
            Focus::Actions => self.focus_pages(),
        }
    }

    fn focus_pages(&mut self) {
        self.focus = Focus::Pages;
        self.message = self
            .language
            .text("Focus: pages.", "焦点：页面。")
            .to_string();
    }

    fn focus_actions(&mut self) {
        self.focus = Focus::Actions;
        self.message = self
            .language
            .text("Focus: actions.", "焦点：动作。")
            .to_string();
    }

    fn activate(&mut self) {
        let action = self.actions().get(self.action_index).copied();
        self.message = match action {
            Some(Action::LaunchCdda) => self.enter_game(),
            Some(Action::InstallGame) => {
                self.open_page(Page::Install);
                self.language
                    .text(
                        "Choose stable or experimental downloads.",
                        "选择稳定版或实验版下载。",
                    )
                    .to_string()
            }
            Some(Action::BackToHome) => {
                self.open_page(Page::Home);
                self.language
                    .text("Returned to Home.", "已返回首页。")
                    .to_string()
            }
            Some(Action::Controls) => {
                self.open_page(Page::Help);
                self.language
                    .text("Opened controls help.", "已打开控制帮助。")
                    .to_string()
            }
            Some(Action::ToggleLanguage) => {
                self.language = self.language.toggle();
                self.config.language = Some(self.language.config_value().to_string());
                self.save_config_message(format!(
                    "{}: {}",
                    self.language.text("Language switched", "语言已切换"),
                    self.language.name()
                ))
            }
            Some(Action::ShowConfigPath) => format!(
                "{}: {}",
                self.language.text("Config file", "配置文件"),
                self.config_path.display()
            ),
            Some(Action::ShowActiveBuild) => {
                let active = if self.config.active_build.is_empty() {
                    self.language.text("none selected", "未选择")
                } else {
                    self.config.active_build.as_str()
                };
                format!(
                    "{}: {}",
                    self.language.text("Active build", "当前版本"),
                    active
                )
            }
            Some(Action::EditGameRoot) => {
                self.open_text_input(SettingField::GameRoot);
                return;
            }
            Some(Action::EditCddaPath) => {
                self.open_text_input(SettingField::CddaPath);
                return;
            }
            Some(Action::EditSteamShortcutName) => {
                self.open_text_input(SettingField::SteamShortcutName);
                return;
            }
            Some(Action::ToggleSteamDeckKonsole) => {
                self.config.use_steam_deck_konsole = !self.config.use_steam_deck_konsole;
                self.save_config_message(format!(
                    "{}: {}",
                    self.language.text("Konsole shortcut", "Konsole 快捷方式"),
                    if self.config.use_steam_deck_konsole {
                        self.language.text("enabled", "已启用")
                    } else {
                        self.language.text("disabled", "已禁用")
                    }
                ))
            }
            Some(Action::ShowSteamShortcutHelp) => {
                let binary = std::env::current_exe()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|_| String::from("cddock"));
                platform_actions::steam_shortcut_report(
                    &binary,
                    &self.config.steam_shortcut_name,
                    self.config.use_steam_deck_konsole,
                )
            }
            Some(Action::SelectStableChannel) => {
                self.config.release_channel = String::from("stable");
                let _ = self.config.save(&self.config_path);
                self.open_release_picker("stable");
                return;
            }
            Some(Action::SelectExperimentalChannel) => {
                self.config.release_channel = String::from("experimental");
                let _ = self.config.save(&self.config_path);
                self.open_release_picker("experimental");
                return;
            }
            Some(Action::CheckNativeDeps) => platform_actions::native_dependency_report(),
            Some(Action::SelectExistingBuild) => {
                self.open_installed_picker();
                return;
            }
            Some(Action::BackupSaves) => self.backup_saves(),
            Some(Action::SearchGuide) => {
                self.open_guide_search();
                return;
            }
            Some(Action::ShowGuideVersion) => match guide::resolve_build(
                &self.game_root(),
                &self.config.active_build,
                &self.config.release_channel,
            ) {
                Ok(build) => format!(
                    "{}: {} / {}",
                    self.language.text("Guide version", "图鉴版本"),
                    build,
                    guide::guide_language(self.language.config_value())
                ),
                Err(error) => error,
            },
            Some(Action::BackToBuilds) => {
                self.open_page(Page::Builds);
                self.builds_dirty = true;
                self.language
                    .text("Returned to versions.", "已返回版本页。")
                    .to_string()
            }
            Some(Action::QuitCddock) => self
                .language
                .text("Quit requested.", "准备退出。")
                .to_string(),
            None => self
                .language
                .text("No action selected.", "未选择动作。")
                .to_string(),
        };
    }

    fn open_page(&mut self, page: Page) {
        if let Some(index) = Page::ALL.iter().position(|candidate| *candidate == page) {
            self.page_index = index;
            self.action_index = 0;
            if page == Page::Builds {
                self.builds_dirty = true;
            }
        }
    }

    fn save_config_message(&self, success_message: String) -> String {
        match self.config.save(&self.config_path) {
            Ok(()) => format!(
                "{} {}.",
                success_message,
                self.language.text("Config saved", "配置已保存")
            ),
            Err(error) => format!(
                "{}: {}",
                self.language.text("Failed to save config", "保存配置失败"),
                error
            ),
        }
    }
}

fn focus_point(focus: Focus, index: usize, page: Page, _action_count: usize) -> FocusPoint {
    match focus {
        Focus::Pages => FocusPoint {
            focus,
            index,
            x: 0,
            y: index as i32,
        },
        Focus::Actions if page == Page::Home => FocusPoint {
            focus,
            index,
            x: index as i32 + 1,
            y: 0,
        },
        Focus::Actions => FocusPoint {
            focus,
            index,
            x: 1,
            y: index as i32,
        },
    }
}

fn spatial_focus_target(
    current: FocusPoint,
    points: &[FocusPoint],
    direction: FocusDirection,
) -> Option<FocusPoint> {
    let directional = points
        .iter()
        .copied()
        .filter(|point| *point != current)
        .filter(|point| match direction {
            FocusDirection::Up => point.y < current.y,
            FocusDirection::Down => point.y > current.y,
            FocusDirection::Left => point.x < current.x,
            FocusDirection::Right => point.x > current.x,
        })
        .collect::<Vec<_>>();
    if directional.is_empty() {
        return None;
    }

    let aligned = directional
        .iter()
        .copied()
        .filter(|point| match direction {
            FocusDirection::Up | FocusDirection::Down => point.x == current.x,
            FocusDirection::Left | FocusDirection::Right => point.y == current.y,
        })
        .collect::<Vec<_>>();
    let candidates = if aligned.is_empty() {
        directional.as_slice()
    } else {
        aligned.as_slice()
    };

    candidates.iter().copied().min_by_key(|point| {
        let forward = match direction {
            FocusDirection::Up => current.y - point.y,
            FocusDirection::Down => point.y - current.y,
            FocusDirection::Left => current.x - point.x,
            FocusDirection::Right => point.x - current.x,
        };
        let perpendicular = match direction {
            FocusDirection::Up | FocusDirection::Down => (point.x - current.x).abs(),
            FocusDirection::Left | FocusDirection::Right => (point.y - current.y).abs(),
        };
        (perpendicular, forward)
    })
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_body(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);

    if let Some(overlay) = &app.overlay {
        match overlay {
            Overlay::Installed(picker) => draw_installed_overlay(frame, area, picker),
            Overlay::ReleaseBrowser(browser) => {
                draw_release_browser(frame, area, browser, app.language)
            }
            Overlay::TextInput(input) => draw_text_input_overlay(frame, area, input, app.language),
        }
    } else if let Some(job) = &app.download {
        draw_download_overlay(frame, area, app.language, job);
    }
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let active = if app.config.active_build.is_empty() {
        app.language.text("none", "未选择")
    } else {
        app.config.active_build.as_str()
    };
    let title = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(
                " CDDock ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(
                app.language
                    .text("  CDDA TUI companion", "  CDDA TUI 管理器"),
            ),
        ]),
        Line::from(vec![
            status_chip("LANG", app.language.name(), Color::Cyan),
            Span::raw("  "),
            status_chip("ROOT", app.config.game_root.as_str(), Color::Gray),
            Span::raw("  "),
            status_chip("CH", app.config.release_channel.as_str(), Color::Yellow),
            Span::raw("  "),
            status_chip("ACTIVE", active, Color::Green),
        ]),
    ])
    .block(Block::default().borders(Borders::ALL))
    .alignment(Alignment::Center);
    frame.render_widget(title, area);
}

fn status_chip<'a>(label: &'static str, value: &'a str, color: Color) -> Span<'a> {
    Span::styled(
        format!("[{label}:{value}]"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

fn draw_body(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(40)])
        .split(area);

    draw_nav(frame, chunks[0], app);
    draw_page(frame, chunks[1], app);
}

fn draw_nav(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let items: Vec<ListItem<'_>> = Page::ALL
        .iter()
        .enumerate()
        .map(|(index, page)| {
            let marker = if index == app.page_index { ">" } else { " " };
            let style = if index == app.page_index && app.focus == Focus::Pages {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if index == app.page_index {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!(
                "{marker} [{}] {}",
                page.icon(),
                page.title(app.language)
            ))
            .style(style)
        })
        .collect();

    let border_style = if app.focus == Focus::Pages {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let nav = List::new(items).block(
        Block::default()
            .title(format!(
                " {}{} ",
                app.language.text("Pages", "页面"),
                if app.focus == Focus::Pages { " *" } else { "" }
            ))
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    frame.render_widget(nav, area);
}

fn draw_page(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let intro_height = if app.page() == Page::Home { 5 } else { 8 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(intro_height), Constraint::Min(8)])
        .split(area);

    let intro = Paragraph::new(page_lines(app))
        .block(
            Block::default()
                .title(format!(
                    " [{}] {} - {} ",
                    app.page().icon(),
                    app.page().title(app.language),
                    app.page().subtitle(app.language)
                ))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(intro, chunks[0]);

    draw_actions(frame, chunks[1], app);
}

fn draw_actions(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.page() == Page::Home {
        draw_home_panel(frame, area, app);
        return;
    }

    if app.page() == Page::Guide
        && let Some(search) = &app.guide_search
    {
        draw_guide_search(frame, area, search, app.language);
        return;
    }

    let items: Vec<ListItem<'_>> = app
        .actions()
        .iter()
        .enumerate()
        .map(|(index, action)| {
            let marker = if index == app.action_index { ">" } else { " " };
            let style = if index == app.action_index && app.focus == Focus::Actions {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if index == app.action_index {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!(
                "{marker} [{:<3}] {}",
                action.badge(),
                action.label(app.language)
            ))
            .style(style)
        })
        .collect();

    let border_style = if app.focus == Focus::Actions {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let actions = List::new(items).block(
        Block::default()
            .title(format!(
                " {}{} ",
                app.language.text("Actions", "动作"),
                if app.focus == Focus::Actions {
                    " *"
                } else {
                    ""
                }
            ))
            .borders(Borders::ALL)
            .border_style(border_style),
    );
    frame.render_widget(actions, area);

    if app.page() == Page::Help {
        draw_help_overlay(frame, area, app.language);
    }
}

fn draw_home_panel(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(12), Constraint::Length(5)])
        .split(area);

    let logo = Paragraph::new(vec![
        Line::from(""),
        Line::from("   ██████╗██████╗ ██████╗  █████╗ "),
        Line::from("  ██╔════╝██╔══██╗██╔══██╗██╔══██╗"),
        Line::from("  ██║     ██║  ██║██║  ██║███████║"),
        Line::from("  ██║     ██║  ██║██║  ██║██╔══██║"),
        Line::from("  ╚██████╗██████╔╝██████╔╝██║  ██║"),
        Line::from("   ╚═════╝╚═════╝ ╚═════╝ ╚═╝  ╚═╝"),
        Line::from(""),
        Line::from("      Cataclysm: Dark Days Ahead"),
    ])
    .block(
        Block::default()
            .title(format!(" {} ", app.language.text("Launch Dock", "启动台")))
            .borders(Borders::ALL)
            .border_style(if app.focus == Focus::Actions {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            }),
    )
    .alignment(Alignment::Center);
    frame.render_widget(logo, chunks[0]);

    let actions = app.actions();
    let columns = actions.len().max(1) as u32;
    let constraints = (0..actions.len())
        .map(|_| Constraint::Ratio(1, columns))
        .collect::<Vec<_>>();
    let action_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(chunks[1]);

    for (index, action) in actions.iter().enumerate() {
        let selected = index == app.action_index && app.focus == Focus::Actions;
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        let button = Paragraph::new(action.label(app.language))
            .block(Block::default().borders(Borders::ALL).border_style(style))
            .alignment(Alignment::Center)
            .style(style);
        frame.render_widget(button, action_chunks[index]);
    }
}

fn draw_installed_overlay(frame: &mut Frame<'_>, area: Rect, picker: &InstalledPicker) {
    let popup = centered_rect(80, 80, area);
    frame.render_widget(Clear, popup);

    let items: Vec<ListItem<'_>> = picker
        .items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let marker = if index == picker.index { ">" } else { " " };
            let style = if index == picker.index {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{marker} {item}")).style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(format!(" {} ", picker.title))
            .borders(Borders::ALL),
    );
    frame.render_widget(list, popup);
}

fn draw_text_input_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    input: &TextInput,
    language: Language,
) {
    let popup = centered_rect(70, 24, area);
    frame.render_widget(Clear, popup);

    let text = vec![
        Line::from(input.value.as_str()),
        Line::from(""),
        Line::from(language.text("Enter saves. Esc cancels.", "Enter 保存，Esc 取消。")),
    ];
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .title(format!(" {} ", input.title))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, popup);
}

fn draw_release_browser(
    frame: &mut Frame<'_>,
    area: Rect,
    browser: &ReleaseBrowser,
    language: Language,
) {
    frame.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);

    let count = browser
        .cache
        .get(&browser.page)
        .map(|cached| cached.items.len())
        .unwrap_or(0);
    let page_hint = if browser.channel == "stable" {
        format!("{count} {}", language.text("stable builds", "个稳定版"))
    } else {
        format!(
            "{} {} | {} {}",
            language.text("page", "第"),
            browser.page,
            count,
            language.text("builds", "个版本"),
        )
    };

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {} ", browser.title),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {page_hint}")),
        if browser.loading {
            Span::styled(
                language.text("  loading...", "  加载中..."),
                Style::default().fg(Color::Yellow),
            )
        } else if browser.has_more {
            Span::styled(
                language.text("  older: ]", "  更旧: ]"),
                Style::default().fg(Color::Green),
            )
        } else {
            Span::raw("")
        },
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(header, chunks[0]);

    let list_area = chunks[1];
    let visible = list_area.height.saturating_sub(2) as usize;
    let items = browser
        .cache
        .get(&browser.page)
        .map(|cached| cached.items.as_slice())
        .unwrap_or(&[]);

    if items.is_empty() {
        let empty_text = if browser.loading {
            language.text(
                "Loading release list... Network and GitHub API can take a while.",
                "正在加载发布列表... 网络和 GitHub API 可能需要一点时间。",
            )
        } else {
            language.text(
                "No compatible builds on this page. Press ] for older releases.",
                "本页没有兼容版本，按 ] 查看更旧版本。",
            )
        };
        let empty = Paragraph::new(empty_text).block(Block::default().borders(Borders::ALL));
        frame.render_widget(empty, list_area);
    } else {
        let scroll_top = browser
            .scroll_top
            .min(items.len().saturating_sub(visible.max(1)));
        let end = (scroll_top + visible).min(items.len());

        let list_items: Vec<ListItem<'_>> = items[scroll_top..end]
            .iter()
            .enumerate()
            .map(|(offset, release)| {
                let index = scroll_top + offset;
                let marker = if index == browser.index { ">" } else { " " };
                let style = if index == browser.index {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(format!("{marker} {}", release.label)).style(style)
            })
            .collect();

        let list =
            List::new(list_items).block(Block::default().borders(Borders::ALL).title(format!(
                " {}-{} / {} ",
                scroll_top + 1,
                end,
                items.len()
            )));
        frame.render_widget(list, list_area);
    }

    let footer = Paragraph::new(language.text(
        "j/k move  [/] prev/next page (older)  Enter install  Esc cancel",
        "j/k 移动  [/] 上/下页(更旧)  Enter 安装  Esc 取消",
    ))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, chunks[2]);
}

fn draw_download_overlay(frame: &mut Frame<'_>, area: Rect, language: Language, job: &DownloadJob) {
    let popup = centered_rect(60, 20, area);
    frame.render_widget(Clear, popup);

    let (label, ratio) = match job.phase() {
        DownloadPhase::Downloading { received, total } => {
            let ratio = total.map(|total| {
                if total == 0 {
                    0.0
                } else {
                    (received as f64 / total as f64).clamp(0.0, 1.0)
                }
            });
            (language.text("Downloading build", "正在下载版本"), ratio)
        }
        DownloadPhase::Extracting => (language.text("Extracting build", "正在解压版本"), None),
        DownloadPhase::Done => (language.text("Install complete", "安装完成"), Some(1.0)),
        DownloadPhase::Failed(_) => (language.text("Install failed", "安装失败"), None),
    };

    let gauge = if let Some(ratio) = ratio {
        Gauge::default()
            .block(Block::default().title(label).borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Cyan))
            .ratio(ratio)
    } else {
        Gauge::default()
            .block(Block::default().title(label).borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Yellow))
            .percent(0)
    };
    frame.render_widget(gauge, popup);
}

fn draw_guide_search(frame: &mut Frame<'_>, area: Rect, search: &GuideSearch, language: Language) {
    frame.render_widget(Clear, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if search.language_note.is_some() { 5 } else { 4 }),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);

    let mut header_lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} ", language.text("Guide Search", "图鉴搜索")),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  [{}] [{}]", search.build, search.language)),
        ]),
        Line::from(vec![
            Span::styled(
                "[Q] ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                if search.query.is_empty() {
                    language.text("type to search", "输入关键词搜索")
                } else {
                    search.query.as_str()
                },
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  |", Style::default().fg(Color::DarkGray)),
            Span::styled(
                language.text(" Enter searches / opens", " Enter 搜索 / 打开"),
                Style::default().fg(Color::Gray),
            ),
        ]),
    ];
    if let Some(note) = &search.language_note {
        header_lines.push(Line::from(vec![
            Span::styled("[LANG] ", Style::default().fg(Color::Yellow)),
            Span::raw(note.clone()),
        ]));
    }

    let header = Paragraph::new(header_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(header, chunks[0]);

    if let Some(detail) = &search.detail {
        draw_guide_detail(
            frame,
            chunks[1],
            detail,
            &search.detail_links,
            search.detail_link_index,
            search.detail_scroll,
            language,
        );
    } else {
        draw_guide_results(frame, chunks[1], search, language);
    }

    let footer_text = if search.detail.is_some() {
        language.text(
            "j/k scroll  Tab link  Enter open link  Esc back",
            "j/k 滚动  Tab 选择关系  Enter 打开关系  Esc 返回",
        )
    } else {
        language.text(
            "type query  Enter search/open  j/k move  Backspace edit  Esc back",
            "输入关键词  Enter 搜索/打开  j/k 移动  Backspace 删除  Esc 返回",
        )
    };
    let footer = Paragraph::new(footer_text).block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, chunks[2]);
}

fn draw_guide_results(frame: &mut Frame<'_>, area: Rect, search: &GuideSearch, language: Language) {
    if search.results.is_empty() {
        let mut lines = vec![Line::from(language.text(
            "Type an id/name/field such as zombie, hammer, rifle, recipe, tileset, then press Enter.",
            "输入 id/名称/字段，例如 zombie、hammer、rifle、recipe、tileset，然后按 Enter。",
        ))];
        if search.language_note.is_some() {
            lines.push(Line::from(""));
            lines.push(Line::from(language.text(
                "This build is using English guide data, so English ids and names work best.",
                "这个版本正在使用英文图鉴数据，优先搜索英文 id 和名称。",
            )));
        }
        let empty = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        frame.render_widget(empty, area);
        return;
    }

    let visible = area.height.saturating_sub(2) as usize;
    let scroll_top = search
        .scroll_top
        .min(search.results.len().saturating_sub(visible.max(1)));
    let end = (scroll_top + visible).min(search.results.len());
    let items: Vec<ListItem<'_>> = search.results[scroll_top..end]
        .iter()
        .enumerate()
        .map(|(offset, item)| {
            let index = scroll_top + offset;
            let marker = if index == search.index { ">" } else { " " };
            let style = if index == search.index {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!(
                "{marker} [{:<14}] {}  {}",
                item.kind, item.id, item.name
            ))
            .style(style)
        })
        .collect();

    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(format!(
        " {}-{} / {} ",
        scroll_top + 1,
        end,
        search.results.len()
    )));
    frame.render_widget(list, area);
}

fn draw_guide_detail(
    frame: &mut Frame<'_>,
    area: Rect,
    detail: &guide::GuideSearchResult,
    links: &[String],
    link_index: usize,
    scroll: u16,
    language: Language,
) {
    let mut lines = vec![
        kv_line("ID", detail.id.clone(), Color::Cyan),
        kv_line("TYPE", detail.kind.clone(), Color::Yellow),
        kv_line("NAME", detail.name.clone(), Color::Green),
    ];
    if !detail.description.is_empty() {
        lines.push(kv_line("DESC", detail.description.clone(), Color::Gray));
    }
    for path in guide_preview_paths(detail)
        .into_iter()
        .take(GUIDE_PREVIEW_LIMIT)
    {
        lines.push(kv_line(
            "SPRITE",
            sprite_preview_label(&path),
            Color::Magenta,
        ));
        lines.extend(render_image_preview_lines(&path, 24, 12));
    }
    push_detail_links(&mut lines, links, link_index);
    push_field_group(&mut lines, "BASIC", detail, BASIC_GUIDE_FIELDS, Color::Cyan);
    push_field_group(&mut lines, "USE", detail, USE_GUIDE_FIELDS, Color::Blue);
    push_field_group(
        &mut lines,
        "COMBAT",
        detail,
        COMBAT_GUIDE_FIELDS,
        Color::Red,
    );
    push_field_group(&mut lines, "FOOD", detail, FOOD_GUIDE_FIELDS, Color::Yellow);
    push_field_group(&mut lines, "MON", detail, MONSTER_GUIDE_FIELDS, Color::Red);
    push_field_group(
        &mut lines,
        "CRAFT",
        detail,
        CRAFT_GUIDE_FIELDS,
        Color::Green,
    );
    push_field_group(&mut lines, "REL", detail, REL_GUIDE_FIELDS, Color::Green);
    push_tile_field_group(&mut lines, detail);
    push_remaining_fields(&mut lines, detail);
    if !detail.raw_json.is_empty() {
        push_raw_json_lines(&mut lines, &detail.raw_json);
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" {} ", language.text("Guide Detail", "图鉴详情")))
                .borders(Borders::ALL),
        )
        .scroll((scroll, 0))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

const BASIC_GUIDE_FIELDS: &[&str] = &[
    "abstract",
    "copy-from",
    "_source_file",
    "looks_like",
    "category",
    "subcategory",
    "symbol",
    "color",
    "volume",
    "weight",
    "longest_side",
    "price",
    "price_postapoc",
    "count",
    "charges",
    "stack_size",
    "material",
    "material_summary",
    "flags",
    "flag_summary",
];
const USE_GUIDE_FIELDS: &[&str] = &[
    "use_action_summary",
    "use_action",
    "ammo",
    "ammo_effects",
    "magazine_well",
    "pocket_summary",
    "pocket_data",
    "container_data",
    "armor_summary",
    "gun_summary",
    "tool_summary",
    "magazine_summary",
    "book_summary",
    "skill_summary",
    "qualities",
    "quality_summary",
    "techniques",
];
const COMBAT_GUIDE_FIELDS: &[&str] = &[
    "melee_summary",
    "technique_summary",
    "range",
    "dispersion",
    "recoil",
    "damage",
    "melee_damage",
    "to_hit",
    "attack_cost",
    "bashing",
    "cutting",
    "armor_bash",
    "armor_cut",
    "armor_bullet",
    "armor_acid",
    "armor_fire",
];
const FOOD_GUIDE_FIELDS: &[&str] = &[
    "comestible_summary",
    "seed_summary",
    "calories",
    "quench",
    "healthy",
    "vitamins",
    "vitamin_summary",
    "comestible_type",
    "fun",
    "addiction_type",
    "spoils_in",
];
const MONSTER_GUIDE_FIELDS: &[&str] = &[
    "hp",
    "speed",
    "aggression",
    "morale",
    "melee_skill",
    "melee_dice",
    "melee_dice_sides",
    "species",
    "biosignature",
    "harvest",
    "death_function",
];
const CRAFT_GUIDE_FIELDS: &[&str] = &[
    "difficulty",
    "skills",
    "proficiencies",
    "proficiency_summary",
    "components",
    "result",
    "byproducts",
    "tools",
    "using",
    "time",
];
const REL_GUIDE_FIELDS: &[&str] = &[
    "crafted_by",
    "used_by_recipe",
    "uncraft_from",
    "uncraft_uses",
    "tool_for_recipe",
    "tool_for_uncraft",
    "byproduct_of_recipe",
    "byproduct_of_uncraft",
    "ammo_used_by",
    "magazine_for",
    "ammo_contained_by",
    "contains_ammo",
    "used_in_construction",
    "tool_for_construction",
    "installed_as_vehicle_part",
    "placed_by_mapgen",
    "placed_by_map_extra",
    "placed_by_overmap_special",
    "referenced_by_eoc",
    "harvested_from",
    "found_in_group",
    "monster_source",
    "monster_group",
    "referenced_by",
];
const TILE_GUIDE_FIELDS: &[&str] = &[
    "tile_match",
    "tiles",
    "tileset",
    "fg",
    "bg",
    "sprite",
    "multitile",
    "additional_tiles",
    "fallback",
];
const GUIDE_PREVIEW_LIMIT: usize = 6;

fn push_field_group(
    lines: &mut Vec<Line<'static>>,
    label: &'static str,
    detail: &guide::GuideSearchResult,
    keys: &[&str],
    color: Color,
) {
    for (key, value) in &detail.fields {
        if keys.iter().any(|candidate| candidate == key) {
            lines.push(kv_line(label, format!("{key}: {value}"), color));
        }
    }
}

fn push_tile_field_group(lines: &mut Vec<Line<'static>>, detail: &guide::GuideSearchResult) {
    for (key, value) in &detail.fields {
        if TILE_GUIDE_FIELDS.iter().any(|candidate| candidate == key) {
            lines.push(kv_line(
                "TILE",
                format!("{key}: {}", tile_display_value(value)),
                Color::Magenta,
            ));
        }
    }
}

fn tile_display_value(value: &str) -> String {
    value
        .split(';')
        .map(str::trim)
        .filter(|part| {
            part.split_once(": ")
                .map(|(key, _)| !key.ends_with("preview"))
                .unwrap_or(true)
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn push_detail_links(lines: &mut Vec<Line<'static>>, links: &[String], link_index: usize) {
    for (index, link) in links.iter().enumerate() {
        let selected = index == link_index;
        let marker = if selected { "> " } else { "  " };
        let style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        lines.push(Line::from(vec![
            Span::styled("[LINK] ", Style::default().fg(Color::Cyan)),
            Span::styled(format!("{marker}{link}"), style),
        ]));
    }
}

fn push_remaining_fields(lines: &mut Vec<Line<'static>>, detail: &guide::GuideSearchResult) {
    for (key, value) in &detail.fields {
        if !is_grouped_guide_field(key) {
            lines.push(kv_line("DATA", format!("{key}: {value}"), Color::Gray));
        }
    }
}

fn push_raw_json_lines(lines: &mut Vec<Line<'static>>, raw_json: &str) {
    for (index, line) in raw_json_display_lines(raw_json).into_iter().enumerate() {
        if index == 0 {
            lines.push(kv_line("RAW", line, Color::DarkGray));
        } else {
            lines.push(kv_line("RAW", format!("  {line}"), Color::DarkGray));
        }
    }
}

fn raw_json_display_lines(raw_json: &str) -> Vec<String> {
    raw_json.lines().map(str::to_string).collect()
}

fn is_grouped_guide_field(key: &str) -> bool {
    [
        BASIC_GUIDE_FIELDS,
        USE_GUIDE_FIELDS,
        COMBAT_GUIDE_FIELDS,
        FOOD_GUIDE_FIELDS,
        MONSTER_GUIDE_FIELDS,
        CRAFT_GUIDE_FIELDS,
        REL_GUIDE_FIELDS,
        TILE_GUIDE_FIELDS,
    ]
    .iter()
    .any(|group| group.iter().any(|candidate| candidate == &key))
}

fn guide_preview_paths(detail: &guide::GuideSearchResult) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for (_, value) in &detail.fields {
        for part in value.split(';') {
            let Some((key, path)) = part.trim().split_once(": ") else {
                continue;
            };
            if !key.ends_with("preview") {
                continue;
            }
            if !path.is_empty() {
                let path = PathBuf::from(path);
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

fn sprite_preview_label(path: &Path) -> String {
    path.file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("local tile")
        .to_string()
}

fn render_image_preview_lines(path: &Path, max_width: u32, max_rows: u32) -> Vec<Line<'static>> {
    let Ok(image) = image::open(path) else {
        return Vec::new();
    };
    let max_height = max_rows.saturating_mul(2).max(1);
    let (source_width, source_height) = image.dimensions();
    let scale = (max_width as f32 / source_width.max(1) as f32)
        .min(max_height as f32 / source_height.max(1) as f32)
        .min(1.0);
    let target_width = ((source_width as f32 * scale).round() as u32).max(1);
    let target_height = ((source_height as f32 * scale).round() as u32).max(1);
    let image = image.resize_exact(target_width, target_height, FilterType::Nearest);
    let (width, height) = image.dimensions();
    let mut lines = Vec::new();
    let mut y = 0;
    while y < height {
        let mut spans = Vec::new();
        spans.push(Span::styled("[IMG] ", Style::default().fg(Color::Magenta)));
        for x in 0..width {
            let upper = image.get_pixel(x, y).to_rgba();
            let lower = if y + 1 < height {
                image.get_pixel(x, y + 1).to_rgba()
            } else {
                image::Rgba([0, 0, 0, 0])
            };
            spans.push(sprite_pixel_span(upper.0, lower.0));
        }
        lines.push(Line::from(spans));
        y += 2;
    }
    lines
}

fn sprite_pixel_span(upper: [u8; 4], lower: [u8; 4]) -> Span<'static> {
    let upper_visible = upper[3] > 16;
    let lower_visible = lower[3] > 16;
    match (upper_visible, lower_visible) {
        (true, true) => Span::styled(
            "▀",
            Style::default()
                .fg(Color::Rgb(upper[0], upper[1], upper[2]))
                .bg(Color::Rgb(lower[0], lower[1], lower[2])),
        ),
        (true, false) => Span::styled(
            "▀",
            Style::default().fg(Color::Rgb(upper[0], upper[1], upper[2])),
        ),
        (false, true) => Span::styled(
            "▄",
            Style::default().fg(Color::Rgb(lower[0], lower[1], lower[2])),
        ),
        (false, false) => Span::raw(" "),
    }
}

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect, language: Language) {
    let popup = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup);

    let text = match language {
        Language::English => vec![
            Line::from("Navigation"),
            Line::from("  tab           switch focus between pages and actions"),
            Line::from("  h/j/k/l       move by screen position"),
            Line::from("  arrows        move by screen position"),
            Line::from("  enter         enter page or activate action"),
            Line::from("  esc           focus pages, then home"),
            Line::from("  q / ctrl-c    quit"),
            Line::from(""),
            Line::from("Steam Deck"),
            Line::from("  Map D-pad or left stick to arrow keys."),
            Line::from("  Map A to Enter, B to Esc, Menu to q if desired."),
            Line::from("  Map L1/R1 to Tab if you still want panel switching."),
            Line::from(""),
            Line::from("Virtual keyboard (Steam + X)"),
            Line::from("  Steam OSK often shows but does not type into raw TUI/Konsole."),
            Line::from("  Use ... > keyboard icon, or map only navigation keys in Steam Input."),
            Line::from("  Guide search may accept pasted text if the OSK sends a paste event."),
            Line::from("  CDDA is launched with SteamDeck=0 to improve in-game typing."),
        ],
        Language::Chinese => vec![
            Line::from("导航"),
            Line::from("  Tab            在页面列表和动作区之间切换焦点"),
            Line::from("  h/j/k/l        按屏幕相对位置移动"),
            Line::from("  方向键          按屏幕相对位置移动"),
            Line::from("  Enter          进入页面或执行动作"),
            Line::from("  Esc            先回页面列表，再回首页"),
            Line::from("  q / Ctrl-C     退出"),
            Line::from(""),
            Line::from("Steam Deck"),
            Line::from("  将十字键或左摇杆映射为方向键。"),
            Line::from("  建议 A 映射 Enter，B 映射 Esc，菜单键映射 q。"),
            Line::from("  若仍需要面板切换，可将 L1/R1 映射为 Tab。"),
            Line::from(""),
            Line::from("虚拟键盘 (Steam + X)"),
            Line::from("  Steam 软键盘常会弹出，但无法向 Konsole/raw TUI 输入字符。"),
            Line::from("  请用 ... > 键盘图标，或在 Steam Input 里只映射导航键。"),
            Line::from("  图鉴搜索若 OSK 发送粘贴事件，可尝试粘贴输入。"),
            Line::from("  启动 CDDA 时会设置 SteamDeck=0，改善游戏内输入。"),
        ],
    };

    let help = Paragraph::new(text)
        .block(
            Block::default()
                .title(format!(" {} ", language.text("Controls", "控制")))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(help, popup);
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let footer = Paragraph::new(Line::from(vec![
        Span::styled(
            " tab ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.language.text("focus  ", "焦点  ")),
        Span::styled(
            " j/k ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.language.text("move  ", "移动  ")),
        Span::styled(
            " enter ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.language.text("select  ", "选择  ")),
        Span::styled(
            " q ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(app.language.text("quit  |  ", "退出  |  ")),
        Span::raw(app.message.as_str()),
    ]))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, area);
}

fn page_lines(app: &App) -> Vec<Line<'static>> {
    let language = app.language;
    let active = if app.config.active_build.is_empty() {
        language.text("none selected", "未选择").to_string()
    } else {
        app.config.active_build.clone()
    };

    let mut lines = match app.page() {
        Page::Home => vec![
            kv_line("BUILD", active, Color::Green),
            kv_line(
                "USER",
                paths::shared_userdata_dir(&app.game_root())
                    .display()
                    .to_string(),
                Color::Cyan,
            ),
            kv_line(
                "FLOW",
                language.text(
                    "Enter game closes any running CDDA, then resumes the last world when available.",
                    "进入游戏会先关闭运行中的 CDDA，并在有记录时继续上次世界。",
                ),
                Color::Yellow,
            ),
        ],
        Page::Builds => {
            let root = app.game_root();
            let userdata = paths::shared_userdata_dir(&root);
            let mut lines = vec![
                kv_line("ROOT", root.display().to_string(), Color::Cyan),
                kv_line("USER", userdata.display().to_string(), Color::Cyan),
                kv_line("ACTIVE", active, Color::Green),
            ];
            if app.installed_builds.is_empty() {
                lines.push(kv_line(
                    "LIST",
                    language.text("no builds installed", "暂无已安装版本"),
                    Color::Yellow,
                ));
            } else {
                for build in app.installed_builds.iter().take(4) {
                    let marker = if build.id == app.config.active_build {
                        "*"
                    } else {
                        " "
                    };
                    lines.push(kv_line(
                        "LIST",
                        format!("{marker} {}", build.id),
                        Color::Gray,
                    ));
                }
                if app.installed_builds.len() > 4 {
                    lines.push(kv_line(
                        "LIST",
                        format!("+{} more", app.installed_builds.len() - 4),
                        Color::DarkGray,
                    ));
                }
            }
            lines
        }
        Page::Install => {
            let root = app.game_root();
            vec![
                kv_line("CHANNEL", app.config.release_channel.clone(), Color::Yellow),
                kv_line(
                    "BINARIES",
                    paths::versions_dir(&root)
                        .join("<build>")
                        .display()
                        .to_string(),
                    Color::Cyan,
                ),
                kv_line(
                    "SHARED",
                    language.text(
                        "userdata/ holds save, config, gfx, mods, sound, etc. for all builds.",
                        "userdata/ 存放所有版本共用的 save、config、gfx、mods、sound 等。",
                    ),
                    Color::Gray,
                ),
                kv_line(
                    "FLOW",
                    language.text(
                        "Fetch list -> download -> extract under versions/.",
                        "获取列表 -> 下载 -> 解压到 versions/。",
                    ),
                    Color::Gray,
                ),
            ]
        }
        Page::Guide => {
            let guide_lang = guide::guide_language(app.language.config_value());
            let guide_build = if app.config.active_build.is_empty() {
                app.language
                    .text("auto from channel", "按通道自动选择")
                    .to_string()
            } else {
                app.config.active_build.clone()
            };
            vec![
                kv_line("SOURCE", "nornagon/cdda-data", Color::Cyan),
                kv_line("BUILD", guide_build, Color::Green),
                kv_line("LANG", guide_lang, Color::Yellow),
                kv_line(
                    "CACHE",
                    guide::cache_summary(&app.game_root()).display().to_string(),
                    Color::Gray,
                ),
            ]
        }
        Page::Settings => vec![
            kv_line("CONFIG", app.config_path.display().to_string(), Color::Gray),
            kv_line("ROOT", app.config.game_root.clone(), Color::Cyan),
            kv_line("CDDA", app.config.cdda_path.clone(), Color::Yellow),
            kv_line(
                "STEAM",
                app.config.steam_shortcut_name.clone(),
                Color::Green,
            ),
            kv_line(
                "KONSOLE",
                if app.config.use_steam_deck_konsole {
                    language.text("enabled", "已启用")
                } else {
                    language.text("disabled", "已禁用")
                },
                Color::Magenta,
            ),
        ],
        Page::Help => vec![
            kv_line(
                "FOCUS",
                language.text("Tab or h/l", "Tab 或 h/l"),
                Color::Cyan,
            ),
            kv_line(
                "MOVE",
                language.text("j/k or arrow keys", "j/k 或方向键"),
                Color::Green,
            ),
            kv_line(
                "DECK",
                language.text(
                    "Map D-pad to arrows, A to Enter, B to Esc.",
                    "十字键映射方向键，A 映射 Enter，B 映射 Esc。",
                ),
                Color::Gray,
            ),
        ],
    };
    lines.push(kv_line("MSG", app.message.clone(), Color::Gray));
    lines
}

fn kv_line<'a>(
    key: &'static str,
    value: impl Into<std::borrow::Cow<'a, str>>,
    color: Color,
) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("[{key}] "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(value),
    ])
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs};

    #[test]
    fn config_round_trips_common_settings() {
        let path =
            std::env::temp_dir().join(format!("cddock-config-test-{}.toml", std::process::id()));
        let config = Config {
            language: Some("chinese".to_string()),
            cdda_path: String::from("~/Games/CDDA-test"),
            game_root: String::from("~/.local/cddock-test"),
            active_build: String::from("experimental-2026-05-22"),
            release_channel: String::from("stable"),
            steam_shortcut_name: String::from("Cataclysm: Dark Days Ahead"),
            use_steam_deck_konsole: false,
            build_channels: HashMap::from([(
                String::from("experimental-2026-05-22"),
                String::from("experimental"),
            )]),
        };

        config.save(&path).expect("save config");
        let loaded = Config::load(&path);

        assert_eq!(loaded.language.as_deref(), Some("chinese"));
        assert_eq!(loaded.cdda_path, "~/Games/CDDA-test");
        assert_eq!(loaded.game_root, "~/.local/cddock-test");
        assert_eq!(loaded.active_build, "experimental-2026-05-22");
        assert_eq!(loaded.release_channel, "stable");
        assert_eq!(loaded.steam_shortcut_name, "Cataclysm: Dark Days Ahead");
        assert!(!loaded.use_steam_deck_konsole);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn config_loads_legacy_build_channel_string() {
        let path = std::env::temp_dir().join(format!(
            "cddock-legacy-config-test-{}.toml",
            std::process::id()
        ));
        fs::write(
            &path,
            r#"
language = "system"
game_root = "~/.local/cddock/gfx"
build_channels = "exp-1=experimental,0.H=stable"
"#,
        )
        .expect("write legacy config");

        let loaded = Config::load(&path);

        assert_eq!(loaded.language, None);
        assert_eq!(loaded.game_root, "~/.local/cddock");
        assert_eq!(
            loaded.build_channels.get("exp-1").map(String::as_str),
            Some("experimental")
        );
        assert_eq!(
            loaded.build_channels.get("0.H").map(String::as_str),
            Some("stable")
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn guide_preview_paths_reads_fg_and_bg_preview_fields() {
        let detail = guide::GuideSearchResult {
            id: "long_pole".to_string(),
            kind: "GENERIC".to_string(),
            name: "long pole".to_string(),
            description: String::new(),
            fields: vec![(
                "tile_match".to_string(),
                concat!(
                    "tileset: Test; ",
                    "fg_preview: /tmp/fg.png; ",
                    "bg_preview: /tmp/bg.png; ",
                    "additional_open_fg_preview: /tmp/open-fg.png; ",
                    "additional_open_bg_preview: /tmp/open-bg.png; ",
                    "fg: 42"
                )
                .to_string(),
            )],
            raw_json: String::new(),
        };

        let paths = guide_preview_paths(&detail);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/tmp/fg.png"),
                PathBuf::from("/tmp/bg.png"),
                PathBuf::from("/tmp/open-fg.png"),
                PathBuf::from("/tmp/open-bg.png")
            ]
        );
    }

    #[test]
    fn guide_preview_limit_shows_layered_tile_states() {
        assert_eq!(GUIDE_PREVIEW_LIMIT, 6);
    }

    #[test]
    fn sprite_pixel_span_leaves_transparent_pixels_blank() {
        let blank = sprite_pixel_span([0, 0, 0, 0], [0, 0, 0, 0]);
        assert_eq!(blank.content.as_ref(), " ");

        let lower = sprite_pixel_span([0, 0, 0, 0], [255, 0, 0, 255]);
        assert_eq!(lower.content.as_ref(), "▄");
    }

    #[test]
    fn tile_display_value_hides_preview_paths() {
        let value =
            "tileset: Test; fg_preview: /tmp/fg.png; fg_crop: 16,16 16x16; bg_preview: /tmp/bg.png";

        let display = tile_display_value(value);
        assert_eq!(display, "tileset: Test; fg_crop: 16,16 16x16");
    }

    #[test]
    fn raw_json_display_lines_keep_full_detail() {
        let raw = format!(
            "{{\n  \"id\": \"long_pole\",\n  \"description\": \"{}\"\n}}",
            "x".repeat(1200)
        );

        let lines = raw_json_display_lines(&raw);

        assert_eq!(lines.len(), 4);
        assert!(lines[2].contains(&"x".repeat(1200)));
        assert!(!lines.iter().any(|line| line.ends_with(" ...")));
    }

    #[test]
    fn spatial_focus_moves_right_to_same_row_action() {
        let points = vec![
            focus_point(Focus::Pages, 2, Page::Install, 4),
            focus_point(Focus::Actions, 0, Page::Install, 4),
            focus_point(Focus::Actions, 1, Page::Install, 4),
            focus_point(Focus::Actions, 2, Page::Install, 4),
            focus_point(Focus::Actions, 3, Page::Install, 4),
        ];

        let next =
            spatial_focus_target(points[0], &points, FocusDirection::Right).expect("right target");
        assert_eq!(next.focus, Focus::Actions);
        assert_eq!(next.index, 2);
    }

    #[test]
    fn spatial_focus_moves_horizontally_across_home_actions() {
        let points = vec![
            focus_point(Focus::Actions, 0, Page::Home, 3),
            focus_point(Focus::Actions, 1, Page::Home, 3),
            focus_point(Focus::Actions, 2, Page::Home, 3),
        ];

        let next =
            spatial_focus_target(points[0], &points, FocusDirection::Right).expect("right target");
        assert_eq!(next.focus, Focus::Actions);
        assert_eq!(next.index, 1);
        assert!(spatial_focus_target(points[0], &points, FocusDirection::Down).is_none());
    }
}

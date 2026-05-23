mod builds;
mod config;
mod guide;
mod install;
mod launch;
mod paths;
mod platform;

use std::{
    io,
    path::PathBuf,
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
use std::collections::HashMap;

use install::{DownloadJob, DownloadPhase, ReleaseOption, fetch_release_page, start_download};
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
        if app.overlay.is_none() {
            app.refresh_installed_builds();
        }
        terminal.draw(|frame| draw(frame, &app))?;

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if app.handle_key(key) => return Ok(()),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    English,
    Chinese,
}

impl Language {
    fn detect() -> Self {
        let locale = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LC_MESSAGES"))
            .or_else(|_| std::env::var("LANG"))
            .unwrap_or_default()
            .to_lowercase();

        if locale.starts_with("zh") {
            Self::Chinese
        } else {
            Self::English
        }
    }

    fn toggle(self) -> Self {
        match self {
            Self::English => Self::Chinese,
            Self::Chinese => Self::English,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Chinese => "中文",
        }
    }

    fn text(self, en: &'static str, zh: &'static str) -> &'static str {
        match self {
            Self::English => en,
            Self::Chinese => zh,
        }
    }

    fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "english" | "en" => Some(Self::English),
            "chinese" | "zh" | "zh-cn" | "zh_hans" => Some(Self::Chinese),
            _ => None,
        }
    }

    fn config_value(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Chinese => "chinese",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Builds,
    Install,
    Guide,
    Settings,
    Help,
}

impl Page {
    const ALL: [Page; 6] = [
        Page::Home,
        Page::Builds,
        Page::Install,
        Page::Guide,
        Page::Settings,
        Page::Help,
    ];

    fn title(self, language: Language) -> &'static str {
        match self {
            Page::Home => language.text("Home", "首页"),
            Page::Builds => language.text("Versions", "版本"),
            Page::Install => language.text("Install", "安装"),
            Page::Guide => language.text("Guide", "图鉴"),
            Page::Settings => language.text("Settings", "设置"),
            Page::Help => language.text("Help", "帮助"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Page::Home => "H",
            Page::Builds => "V",
            Page::Install => "+",
            Page::Guide => "G",
            Page::Settings => "*",
            Page::Help => "?",
        }
    }

    fn subtitle(self, language: Language) -> &'static str {
        match self {
            Page::Home => language.text("Status and quick launch", "状态和快速启动"),
            Page::Builds => language.text(
                "Installed builds and active version",
                "已安装版本和当前版本",
            ),
            Page::Install => {
                language.text("Choose release channel and download", "选择发布通道并下载")
            }
            Page::Guide => language.text(
                "Search cdda-guide data with local cache",
                "搜索 cdda-guide 数据并本地缓存",
            ),
            Page::Settings => language.text("Language, paths, and controls", "语言、路径和控制"),
            Page::Help => language.text(
                "Keyboard and Steam Input mapping",
                "键盘和 Steam Input 映射",
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    LaunchCdda,
    QuickResume,
    InstallGame,
    SelectStableChannel,
    SelectExperimentalChannel,
    BackToBuilds,
    SelectExistingBuild,
    SearchGuide,
    ShowGuideVersion,
    ShowActiveBuild,
    ToggleLanguage,
    ShowConfigPath,
    SteamShortcutName,
    Controls,
    BackToHome,
    QuitCddock,
}

impl Action {
    fn label(self, language: Language) -> &'static str {
        match self {
            Self::LaunchCdda => language.text("Launch CDDA", "启动 CDDA"),
            Self::QuickResume => language.text("Quick return to last world", "快速回到上次世界"),
            Self::InstallGame => language.text("Install game", "安装游戏"),
            Self::SelectStableChannel => language.text("Fetch stable list", "获取稳定版列表"),
            Self::SelectExperimentalChannel => {
                language.text("Fetch experimental list", "获取实验版列表")
            }
            Self::BackToBuilds => language.text("Back to versions", "返回版本页"),
            Self::SelectExistingBuild => language.text("Switch build", "切换版本"),
            Self::SearchGuide => language.text("Search guide data", "搜索图鉴数据"),
            Self::ShowGuideVersion => language.text("Show guide version", "查看图鉴版本"),
            Self::ShowActiveBuild => language.text("Show active build", "查看当前版本"),
            Self::ToggleLanguage => language.text("Switch language", "切换语言"),
            Self::ShowConfigPath => language.text("Show config path", "显示配置路径"),
            Self::SteamShortcutName => language.text("Steam shortcut name", "Steam 快捷方式名称"),
            Self::Controls => language.text("Controls", "控制"),
            Self::BackToHome => language.text("Back to Home", "返回首页"),
            Self::QuitCddock => language.text("Quit CDDock", "退出 CDDock"),
        }
    }

    fn badge(self) -> &'static str {
        match self {
            Self::LaunchCdda | Self::QuickResume => "RUN",
            Self::SearchGuide | Self::ShowGuideVersion => "GDE",
            Self::ToggleLanguage
            | Self::ShowConfigPath
            | Self::SteamShortcutName
            | Self::Controls => "SET",
            Self::InstallGame | Self::SelectStableChannel | Self::SelectExperimentalChannel => {
                "GET"
            }
            Self::BackToBuilds | Self::BackToHome => "NAV",
            Self::SelectExistingBuild | Self::ShowActiveBuild => "USE",
            Self::QuitCddock => "EXT",
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

#[derive(Debug)]
struct ReleaseBrowser {
    channel: String,
    title: String,
    page: u32,
    index: usize,
    scroll_top: usize,
    has_more: bool,
    cache: HashMap<u32, Vec<ReleaseOption>>,
    loading: bool,
}

#[derive(Debug)]
struct ReleaseFetchJob {
    channel: String,
    page: u32,
    receiver: Receiver<Result<install::ReleasePage, String>>,
}

#[derive(Debug)]
struct LaunchPicker {
    items: Vec<String>,
    worlds: Vec<Option<String>>,
    index: usize,
}

#[derive(Debug)]
struct GuideSearch {
    query: String,
    build: String,
    language: String,
    results: Vec<guide::GuideSearchResult>,
    index: usize,
    scroll_top: usize,
    detail: Option<guide::GuideSearchResult>,
}

#[derive(Debug)]
enum Overlay {
    Installed(InstalledPicker),
    ReleaseBrowser(ReleaseBrowser),
    Launch(LaunchPicker),
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
        let _ = paths::migrate_legacy_layout(&game_root, &channel);
        let _ = config.save(&config_path);

        Self {
            config,
            config_path,
            language,
            focus: Focus::Pages,
            page_index: 0,
            action_index: 0,
            message: language
                .text(
                    "Ready. Tab changes focus. j/k or arrows move in the focused panel.",
                    "已就绪。Tab 切换焦点，j/k 或方向键在当前面板内移动。",
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
        browser.cache.insert(page.page, page.items);

        if browser
            .cache
            .get(&browser.page)
            .is_some_and(|items| items.is_empty())
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
                    .map(|items| items.len())
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
            KeyCode::Char('h') | KeyCode::Left => self.focus_pages(),
            KeyCode::Char('l') | KeyCode::Right => self.focus_actions(),
            KeyCode::Char('k') | KeyCode::Up => self.previous_item(),
            KeyCode::Char('j') | KeyCode::Down => self.next_item(),
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
            Some(Overlay::Launch(picker)) => match key.code {
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
                KeyCode::Enter => self.confirm_launch_picker(),
                _ => {}
            },
            None => {}
        }
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
                    if let Some(search) = self.guide_search.as_mut() {
                        search.detail = None;
                    }
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
                }
            }
            KeyCode::Char('q') => return true,
            KeyCode::Char('k') | KeyCode::Up => self.guide_move_up(),
            KeyCode::Char('j') | KeyCode::Down => self.guide_move_down(),
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
        let Some(items) = browser.cache.get(&browser.page) else {
            self.overlay = Some(Overlay::ReleaseBrowser(browser));
            return;
        };
        let Some(release) = items.get(browser.index).cloned() else {
            self.message = self
                .language
                .text("No release selected.", "未选择发布版本。")
                .to_string();
            return;
        };
        self.start_release_download(release);
    }

    fn open_launch_picker(&mut self) {
        let Some((_, userdata)) = self.active_build_and_userdata() else {
            self.message = self
                .language
                .text(
                    "No active build selected. Choose one under Versions.",
                    "未选择当前版本，请先在版本页选择。",
                )
                .to_string();
            return;
        };

        let last_world = builds::find_most_recent_world(&userdata);
        let mut items = vec![self.language.text("Enter game", "进入游戏").to_string()];
        let mut worlds = vec![None];
        if let Some(world) = last_world {
            items.push(format!(
                "{}: {world}",
                self.language
                    .text("Enter last played world", "进入最后游玩的世界")
            ));
            worlds.push(Some(world));
        }

        self.overlay = Some(Overlay::Launch(LaunchPicker {
            items,
            worlds,
            index: 0,
        }));
        self.message = self
            .language
            .text("Choose how to launch CDDA.", "选择启动 CDDA 的方式。")
            .to_string();
    }

    fn confirm_launch_picker(&mut self) {
        let Some(Overlay::Launch(picker)) = self.overlay.take() else {
            return;
        };
        let world = picker.worlds.get(picker.index).cloned().flatten();
        self.message = self.launch_active_build(world.as_deref());
    }

    fn active_build_and_userdata(&self) -> Option<(PathBuf, PathBuf)> {
        let path = builds::active_build_path(&self.game_root(), &self.config.active_build)?;
        let channel = self.config.channel_for_build(&self.config.active_build);
        let userdata = paths::userdata_dir(&self.game_root(), &channel);
        Some((path, userdata))
    }

    fn launch_active_build(&mut self, world: Option<&str>) -> String {
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

        match launch::launch_build(&path, &userdata, world) {
            Ok(pid) => {
                self.game_pid = Some(pid);
                if let Some(world) = world {
                    format!(
                        "{}: {world} (pid {pid})",
                        self.language
                            .text("Launched last played world", "已进入最后游玩的世界")
                    )
                } else {
                    format!(
                        "{} (pid {pid})",
                        self.language.text("Launched CDDA", "已启动 CDDA")
                    )
                }
            }
            Err(error) => error,
        }
    }

    fn quick_resume_last_world(&mut self) -> String {
        let Some((_, userdata)) = self.active_build_and_userdata() else {
            return self
                .language
                .text(
                    "No active build selected. Choose one under Versions.",
                    "未选择当前版本，请先在版本页选择。",
                )
                .to_string();
        };
        let Some(world) = builds::find_most_recent_world(&userdata) else {
            return self
                .language
                .text("No last played world found.", "没有找到最后游玩的世界。")
                .to_string();
        };

        if let Some(pid) = self.game_pid.take() {
            let _ = launch::stop_game(pid);
        }
        self.launch_active_build(Some(&world))
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
                    results: Vec::new(),
                    index: 0,
                    scroll_top: 0,
                    detail: None,
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
        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if search.detail.is_some() {
            return;
        }
        if let Some(result) = search.results.get(search.index).cloned() {
            search.detail = Some(result);
            return;
        }

        let query = search.query.clone();
        let build = search.build.clone();
        let language = search.language.clone();
        let cache_key_matches =
            self.guide_dataset
                .as_ref()
                .is_some_and(|(cached_build, cached_language, _)| {
                    cached_build == &build && cached_language == &language
                });
        if !cache_key_matches {
            match guide::load_dataset(&self.game_root(), &build, &language) {
                Ok(dataset) => {
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
        }
        self.message = format!(
            "{}: {count}",
            self.language.text("Guide results", "图鉴结果")
        );
    }

    fn guide_move_up(&mut self) {
        let Some(search) = self.guide_search.as_mut() else {
            return;
        };
        if search.detail.is_some() || search.results.is_empty() {
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
        if search.detail.is_some() || search.results.is_empty() {
            return;
        }
        search.index = (search.index + 1) % search.results.len();
        const VIEWPORT: usize = 18;
        if search.index + 1 >= search.scroll_top + VIEWPORT {
            search.scroll_top = search.index.saturating_sub(VIEWPORT - 1);
        }
    }

    fn browser_items(&self) -> Option<&Vec<ReleaseOption>> {
        match self.overlay.as_ref()? {
            Overlay::ReleaseBrowser(browser) => browser.cache.get(&browser.page),
            Overlay::Installed(_) | Overlay::Launch(_) => None,
        }
    }

    fn browser_move_up(&mut self) {
        let Some(items) = self.browser_items().cloned() else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut() else {
            return;
        };
        browser.index = browser.index.checked_sub(1).unwrap_or(items.len() - 1);
        if browser.index < browser.scroll_top {
            browser.scroll_top = browser.index;
        }
    }

    fn browser_move_down(&mut self) {
        let Some(items) = self.browser_items().cloned() else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let Some(Overlay::ReleaseBrowser(browser)) = self.overlay.as_mut() else {
            return;
        };
        browser.index = (browser.index + 1) % items.len();
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
            browser.has_more = browser.cache.contains_key(&(page + 1));
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

    fn previous_item(&mut self) {
        match self.focus {
            Focus::Pages => self.previous_page(),
            Focus::Actions => self.previous_action(),
        }
    }

    fn next_item(&mut self) {
        match self.focus {
            Focus::Pages => self.next_page(),
            Focus::Actions => self.next_action(),
        }
    }

    fn previous_page(&mut self) {
        self.page_index = self
            .page_index
            .checked_sub(1)
            .unwrap_or(Page::ALL.len().saturating_sub(1));
        self.action_index = 0;
        self.on_page_changed();
    }

    fn next_page(&mut self) {
        self.page_index = (self.page_index + 1) % Page::ALL.len();
        self.action_index = 0;
        self.on_page_changed();
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

    fn previous_action(&mut self) {
        let actions = self.actions();
        if actions.is_empty() {
            self.action_index = 0;
            return;
        }
        self.action_index = self
            .action_index
            .checked_sub(1)
            .unwrap_or(actions.len().saturating_sub(1));
    }

    fn next_action(&mut self) {
        let actions = self.actions();
        if actions.is_empty() {
            self.action_index = 0;
            return;
        }
        self.action_index = (self.action_index + 1) % actions.len();
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
            Some(Action::LaunchCdda) => {
                self.open_launch_picker();
                return;
            }
            Some(Action::QuickResume) => self.quick_resume_last_world(),
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
            Some(Action::SteamShortcutName) => format!(
                "{}: {}",
                self.language
                    .text("Steam shortcut name", "Steam 快捷方式名称"),
                self.config.steam_shortcut_name
            ),
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
            Some(Action::SelectExistingBuild) => {
                self.open_installed_picker();
                return;
            }
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

fn page_actions(page: Page) -> &'static [Action] {
    match page {
        Page::Home => &[
            Action::SelectExistingBuild,
            Action::LaunchCdda,
            Action::QuickResume,
            Action::QuitCddock,
        ],
        Page::Builds => &[
            Action::InstallGame,
            Action::SelectExistingBuild,
            Action::ShowActiveBuild,
        ],
        Page::Install => &[
            Action::SelectStableChannel,
            Action::SelectExperimentalChannel,
            Action::BackToBuilds,
        ],
        Page::Guide => &[
            Action::SearchGuide,
            Action::ShowGuideVersion,
            Action::BackToHome,
        ],
        Page::Settings => &[
            Action::ToggleLanguage,
            Action::ShowConfigPath,
            Action::SteamShortcutName,
            Action::Controls,
        ],
        Page::Help => &[Action::BackToHome],
    }
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
            Overlay::Launch(picker) => draw_launch_overlay(frame, area, picker, app.language),
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
        Line::from("        ______  ______  ______   ______  "),
        Line::from("       / ____/ / __  / / __  /  / ____/  "),
        Line::from("      / /     / / / / / / / /  / /_      "),
        Line::from("     / /___  / /_/ / / /_/ /  / __/      "),
        Line::from("     \\____/ /_____/ /_____/  /_/         "),
        Line::from(""),
        Line::from("        Cataclysm: Dark Days Ahead"),
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

fn draw_launch_overlay(
    frame: &mut Frame<'_>,
    area: Rect,
    picker: &LaunchPicker,
    language: Language,
) {
    let popup = centered_rect(58, 30, area);
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
            .title(format!(" {} ", language.text("Launch CDDA", "启动 CDDA")))
            .borders(Borders::ALL),
    );
    frame.render_widget(list, popup);
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
        .map(|items| items.len())
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
        .cloned()
        .unwrap_or_default();

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
            Constraint::Length(4),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(area);

    let header = Paragraph::new(vec![
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
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    frame.render_widget(header, chunks[0]);

    if let Some(detail) = &search.detail {
        draw_guide_detail(frame, chunks[1], detail, language);
    } else {
        draw_guide_results(frame, chunks[1], search, language);
    }

    let footer = Paragraph::new(language.text(
        "type query  Enter search/open  j/k move  Backspace edit  Esc back",
        "输入关键词  Enter 搜索/打开  j/k 移动  Backspace 删除  Esc 返回",
    ))
    .block(Block::default().borders(Borders::ALL));
    frame.render_widget(footer, chunks[2]);
}

fn draw_guide_results(frame: &mut Frame<'_>, area: Rect, search: &GuideSearch, language: Language) {
    if search.results.is_empty() {
        let empty = Paragraph::new(language.text(
            "Type an id/name such as mon_sewer_fish, zombie, hammer, then press Enter.",
            "输入 id/名称，例如 mon_sewer_fish、zombie、hammer，然后按 Enter。",
        ))
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
    lines.push(kv_line(
        "TILE",
        language.text("tile preview is planned", "贴图预览待接入"),
        Color::DarkGray,
    ));
    for (key, value) in &detail.fields {
        lines.push(kv_line("DATA", format!("{key}: {value}"), Color::Gray));
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" {} ", language.text("Guide Detail", "图鉴详情")))
                .borders(Borders::ALL),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_help_overlay(frame: &mut Frame<'_>, area: Rect, language: Language) {
    let popup = centered_rect(70, 70, area);
    frame.render_widget(Clear, popup);

    let text = match language {
        Language::English => vec![
            Line::from("Navigation"),
            Line::from("  tab           switch focus between pages and actions"),
            Line::from("  h / left      focus pages"),
            Line::from("  l / right     focus actions"),
            Line::from("  k / up        previous item in focused panel"),
            Line::from("  j / down      next item in focused panel"),
            Line::from("  enter         enter page or activate action"),
            Line::from("  esc           focus pages, then home"),
            Line::from("  q / ctrl-c    quit"),
            Line::from(""),
            Line::from("Steam Deck"),
            Line::from("  Map D-pad or left stick to arrow keys."),
            Line::from("  Map A to Enter, B to Esc, Menu to q if desired."),
            Line::from("  Map L1/R1 to Tab or h/l for focus switching."),
        ],
        Language::Chinese => vec![
            Line::from("导航"),
            Line::from("  Tab            在页面列表和动作区之间切换焦点"),
            Line::from("  h / 左方向键    焦点移到页面列表"),
            Line::from("  l / 右方向键    焦点移到动作区"),
            Line::from("  k / 上方向键    当前面板上一个项目"),
            Line::from("  j / 下方向键    当前面板下一个项目"),
            Line::from("  Enter          进入页面或执行动作"),
            Line::from("  Esc            先回页面列表，再回首页"),
            Line::from("  q / Ctrl-C     退出"),
            Line::from(""),
            Line::from("Steam Deck"),
            Line::from("  将十字键或左摇杆映射为方向键。"),
            Line::from("  建议 A 映射 Enter，B 映射 Esc，菜单键映射 q。"),
            Line::from("  建议 L1/R1 映射 Tab 或 h/l，用于切换焦点。"),
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
                "FLOW",
                language.text(
                    "Switch build, launch, or quick return to the last world.",
                    "切换版本、启动游戏，或快速回到上次世界。",
                ),
                Color::Yellow,
            ),
        ],
        Page::Builds => {
            let root = app.game_root();
            let channel = if app.config.active_build.is_empty() {
                app.config.release_channel.clone()
            } else {
                app.config.channel_for_build(&app.config.active_build)
            };
            let userdata = paths::userdata_dir(&root, &channel);
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
                        "userdata-<channel>/ holds save, gfx, mods, etc.",
                        "userdata-<通道>/ 存放 save、gfx、mods 等。",
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
            kv_line(
                "STEAM",
                app.config.steam_shortcut_name.clone(),
                Color::Green,
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
}

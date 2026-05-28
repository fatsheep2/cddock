#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Chinese,
}

impl Language {
    pub fn detect() -> Self {
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

    pub fn toggle(self) -> Self {
        match self {
            Self::English => Self::Chinese,
            Self::Chinese => Self::English,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Chinese => "中文",
        }
    }

    pub fn text(self, en: &'static str, zh: &'static str) -> &'static str {
        match self {
            Self::English => en,
            Self::Chinese => zh,
        }
    }

    pub fn from_config_value(value: &str) -> Option<Self> {
        match value {
            "english" | "en" => Some(Self::English),
            "chinese" | "zh" | "zh-cn" | "zh_hans" => Some(Self::Chinese),
            _ => None,
        }
    }

    pub fn config_value(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Chinese => "chinese",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Builds,
    Install,
    Guide,
    Settings,
    Help,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Home,
        Page::Builds,
        Page::Install,
        Page::Guide,
        Page::Settings,
        Page::Help,
    ];

    pub fn title(self, language: Language) -> &'static str {
        match self {
            Page::Home => language.text("Home", "首页"),
            Page::Builds => language.text("Versions", "版本"),
            Page::Install => language.text("Install", "安装"),
            Page::Guide => language.text("Guide", "图鉴"),
            Page::Settings => language.text("Settings", "设置"),
            Page::Help => language.text("Help", "帮助"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Page::Home => "H",
            Page::Builds => "V",
            Page::Install => "+",
            Page::Guide => "G",
            Page::Settings => "*",
            Page::Help => "?",
        }
    }

    pub fn subtitle(self, language: Language) -> &'static str {
        match self {
            Page::Home => language.text(
                "Switch build and enter game (one instance)",
                "切换版本并进入游戏（单进程）",
            ),
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
pub enum Action {
    LaunchCdda,
    InstallGame,
    SelectStableChannel,
    SelectExperimentalChannel,
    BackToBuilds,
    SelectExistingBuild,
    BackupSaves,
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
    pub fn label(self, language: Language) -> &'static str {
        match self {
            Self::LaunchCdda => language.text("Enter game", "进入游戏"),
            Self::InstallGame => language.text("Install game", "安装游戏"),
            Self::SelectStableChannel => language.text("Fetch stable list", "获取稳定版列表"),
            Self::SelectExperimentalChannel => {
                language.text("Fetch experimental list", "获取实验版列表")
            }
            Self::BackToBuilds => language.text("Back to versions", "返回版本页"),
            Self::SelectExistingBuild => language.text("Switch build", "切换版本"),
            Self::BackupSaves => language.text("Backup saves", "备份存档"),
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

    pub fn badge(self) -> &'static str {
        match self {
            Self::LaunchCdda => "RUN",
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
            Self::BackupSaves => "BAK",
            Self::QuitCddock => "EXT",
        }
    }
}

pub fn page_actions(page: Page) -> &'static [Action] {
    match page {
        Page::Home => &[
            Action::SelectExistingBuild,
            Action::LaunchCdda,
            Action::QuitCddock,
        ],
        Page::Builds => &[
            Action::InstallGame,
            Action::SelectExistingBuild,
            Action::ShowActiveBuild,
            Action::BackupSaves,
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

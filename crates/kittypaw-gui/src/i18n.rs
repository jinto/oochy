use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    Ko,
    En,
}

impl Locale {
    pub fn from_str(s: &str) -> Self {
        match s {
            "en" => Self::En,
            _ => Self::Ko,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ko => "ko",
            Self::En => "en",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Ko => "한국어",
            Self::En => "English",
        }
    }
}

pub struct I18n {
    strings: HashMap<&'static str, [&'static str; 2]>, // [ko, en]
    pub locale: Locale,
}

impl I18n {
    pub fn new(locale: Locale) -> Self {
        let mut strings = HashMap::new();

        // key => [Korean, English]

        // Sidebar
        strings.insert("nav.chat", ["채팅", "Chat"]);
        strings.insert("nav.dashboard", ["상황판", "Dashboard"]);
        strings.insert("nav.skills", ["스킬", "Skills"]);
        strings.insert("nav.settings", ["설정", "Settings"]);

        // Chat
        strings.insert(
            "chat.greeting",
            ["무엇을 도와드릴까요?", "How can I help you?"],
        );
        strings.insert(
            "chat.subtitle",
            [
                "KittyPaw는 당신의 AI 에이전트입니다.",
                "KittyPaw is your AI agent.",
            ],
        );
        strings.insert(
            "chat.input_placeholder",
            ["메시지를 입력하세요...", "Message KittyPaw..."],
        );
        strings.insert("chat.send", ["보내기", "Send"]);
        strings.insert("chat.thinking", ["생각하는 중...", "Thinking..."]);
        strings.insert("chat.you", ["나", "You"]);
        strings.insert(
            "chat.no_llm",
            [
                "LLM이 설정되지 않았습니다. 설정에서 API 키를 입력해주세요.",
                "No LLM configured. Please set your API key in Settings.",
            ],
        );

        // Quick prompts
        strings.insert("quick.who", ["🐱 너는 누구니?", "🐱 Who are you?"]);
        strings.insert(
            "quick.who_prompt",
            [
                "너는 누구니? 어떤 에이전트인지 소개해줘.",
                "Who are you? Tell me about yourself.",
            ],
        );
        strings.insert(
            "quick.what",
            ["🛠 어떤 일을 할 수 있어?", "🛠 What can you do?"],
        );
        strings.insert(
            "quick.what_prompt",
            [
                "너는 어떤 일을 할 수 있어? 구체적으로 알려줘.",
                "What can you do? Be specific.",
            ],
        );
        strings.insert(
            "quick.status",
            [
                "📋 지금 무슨 일을 하고 있어?",
                "📋 What are you working on?",
            ],
        );
        strings.insert(
            "quick.status_prompt",
            [
                "지금 어떤 스킬들이 등록되어 있고, 무슨 일을 하고 있어?",
                "What skills are registered and what are you doing?",
            ],
        );
        strings.insert(
            "quick.new_skill",
            ["✨ 새 스킬 만들어줘", "✨ Create a new skill"],
        );
        strings.insert(
            "quick.new_skill_prompt",
            [
                "새로운 스킬을 만들고 싶어. 어떻게 시작하면 돼?",
                "I want to create a new skill. How do I start?",
            ],
        );

        // Dashboard
        strings.insert("dash.active_skills", ["활성 스킬", "Active Skills"]);
        strings.insert("dash.today_runs", ["오늘 실행", "Today's Runs"]);
        strings.insert("dash.today_tokens", ["오늘 토큰", "Today's Tokens"]);
        strings.insert("dash.silent_opt", ["자동 최적화", "Silent Optimizations"]);
        strings.insert("dash.recent", ["최근 활동", "Recent Activity"]);
        strings.insert(
            "dash.empty",
            [
                "아직 실행 기록이 없습니다. 스킬을 설치하고 실행해보세요.",
                "No activity yet. Install and run skills to get started.",
            ],
        );

        // Settings
        strings.insert("settings.title", ["설정", "Settings"]);
        strings.insert("settings.language", ["언어", "Language"]);
        strings.insert("settings.api_key", ["API 키", "API Key"]);
        strings.insert("settings.save", ["저장", "Save"]);
        strings.insert("settings.saved", ["저장 완료", "Saved"]);
        strings.insert("settings.local_model", ["로컬 모델 연결", "Local Model"]);
        strings.insert("settings.server_url", ["모델 서버 URL", "Model Server URL"]);
        strings.insert("settings.model_name", ["모델 이름", "Model Name"]);
        strings.insert(
            "settings.telegram",
            ["채널 연결 (Telegram)", "Channel (Telegram)"],
        );
        strings.insert("settings.bot_token", ["봇 토큰", "Bot Token"]);
        strings.insert("settings.chat_id", ["채팅 ID", "Chat ID"]);
        strings.insert("settings.search", ["웹 검색", "Web Search"]);
        strings.insert("settings.search_backend", ["검색 백엔드", "Search Backend"]);
        strings.insert("settings.search_api_key", ["검색 API 키", "Search API Key"]);
        strings.insert(
            "settings.search_desc",
            [
                "기본값(DuckDuckGo)은 API 키 없이 작동합니다. 더 나은 품질이 필요하면 유료 API를 설정하세요.",
                "Default (DuckDuckGo) works without an API key. Set a paid API for better quality.",
            ],
        );
        strings.insert("settings.slack", ["채널 연결 (Slack)", "Channel (Slack)"]);
        strings.insert(
            "settings.discord",
            ["채널 연결 (Discord)", "Channel (Discord)"],
        );

        // Skill gallery
        strings.insert("skills.installed", ["설치됨", "Installed"]);
        strings.insert("skills.store", ["스토어", "Store"]);
        strings.insert("skills.search", ["스킬 검색...", "Search skills..."]);
        strings.insert(
            "skills.no_found",
            ["스킬을 찾을 수 없습니다.", "No skills found."],
        );
        strings.insert("skills.install", ["설치", "Install"]);
        strings.insert("skills.installing", ["설치 중...", "Installing..."]);

        // Onboarding
        strings.insert("onboard.start", ["시작하기", "Get Started"]);
        strings.insert("onboard.ready", ["준비 완료!", "All set!"]);
        strings.insert(
            "onboard.desc",
            [
                "채팅에서 자유롭게 대화하거나, 스킬을 설치하고 실행해보세요",
                "Start chatting or install skills to automate tasks",
            ],
        );

        Self { strings, locale }
    }

    pub fn t(&self, key: &str) -> &'static str {
        match self.strings.get(key) {
            Some(pair) => match self.locale {
                Locale::Ko => pair[0],
                Locale::En => pair[1],
            },
            None => "???",
        }
    }
}

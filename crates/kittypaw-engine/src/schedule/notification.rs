/// Caches HTTP client and channel credentials; sends notifications without
/// re-reading secrets or re-allocating a client on every call.
pub struct NotificationSender {
    client: reqwest::Client,
    telegram: Option<(String, String)>, // (token, chat_id)
    slack: Option<(String, String)>,    // (token, channel)
}

impl NotificationSender {
    pub fn new(config: &kittypaw_core::config::Config) -> Self {
        let tg_token = kittypaw_core::credential::resolve_credential(
            "telegram",
            "telegram_token",
            "KITTYPAW_TELEGRAM_TOKEN",
            config,
        )
        // Also try {channel}/bot_token for GUI onboarding path
        .or_else(|| {
            kittypaw_core::secrets::get_secret("telegram", "bot_token")
                .ok()
                .flatten()
                .filter(|s| !s.is_empty())
        });
        let tg_chat_id = kittypaw_core::credential::resolve_credential(
            "telegram",
            "chat_id",
            "KITTYPAW_TELEGRAM_CHAT_ID",
            config,
        );
        let telegram = match (tg_token, tg_chat_id) {
            (Some(token), Some(chat_id)) => Some((token, chat_id)),
            _ => None,
        };

        let slack_token = kittypaw_core::credential::resolve_credential(
            "slack",
            "slack_token",
            "KITTYPAW_SLACK_TOKEN",
            config,
        );
        let slack_channel = kittypaw_core::credential::resolve_credential(
            "slack",
            "slack_channel",
            "KITTYPAW_SLACK_CHANNEL",
            config,
        );
        let slack = match (slack_token, slack_channel) {
            (Some(token), Some(channel)) => Some((token, channel)),
            _ => None,
        };

        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            telegram,
            slack,
        }
    }

    pub fn send(&self, message: &str) {
        if let Some((token, chat_id)) = &self.telegram {
            let url = format!("https://api.telegram.org/bot{}/sendMessage", token);
            let body =
                serde_json::json!({"chat_id": chat_id, "text": message, "parse_mode": "Markdown"});
            let client = self.client.clone();
            tokio::spawn(async move {
                if let Err(e) = client.post(&url).json(&body).send().await {
                    tracing::warn!("Notification failed: {e}");
                }
            });
            return;
        }
        if let Some((token, channel)) = &self.slack {
            let body = serde_json::json!({"channel": channel, "text": message});
            let client = self.client.clone();
            let token = token.clone();
            tokio::spawn(async move {
                if let Err(e) = client
                    .post("https://slack.com/api/chat.postMessage")
                    .header("Authorization", format!("Bearer {}", token))
                    .json(&body)
                    .send()
                    .await
                {
                    tracing::warn!("Slack notification failed: {e}");
                }
            });
        }
    }

    pub fn notify_recovery(&self, name: &str) {
        self.send(&format!(
            "🔧 *{}* 자동 복구됨\n실패 후 자동 재시도로 정상 작동 중입니다.",
            name
        ));
    }

    pub fn notify_patterns(&self, name: &str, patterns: &[(String, String)]) {
        let list: Vec<String> = patterns
            .iter()
            .map(|(k, v)| format!("  → {} = {}", k, v))
            .collect();
        self.send(&format!(
            "📊 *{}* 패턴 감지\n반복 사용된 값을 기본값으로 설정했습니다:\n{}",
            name,
            list.join("\n")
        ));
    }

    pub fn notify_retry(&self, name: &str, failures: u32, delay_secs: u64) {
        self.send(&format!(
            "⏳ *{}* 재시도 예정\n{}초 후 자동 재시도합니다 (시도 {}/3).",
            name, delay_secs, failures
        ));
    }

    pub fn notify_fix_applied(&self, name: &str, error: &str, fix_id: i64) {
        self.send(&format!(
            "🔧 *{}* 자동 수정 적용됨\n에러: {}\n`kittypaw fixes show {}`",
            name,
            error.chars().take(100).collect::<String>(),
            fix_id
        ));
    }

    pub fn notify_fix_pending(&self, name: &str, error: &str, fix_id: i64) {
        self.send(&format!(
            "🔧 *{}* 수정안 생성됨 (승인 대기)\n에러: {}\n`kittypaw fixes approve {}`",
            name,
            error.chars().take(100).collect::<String>(),
            fix_id
        ));
    }

    pub fn notify_reflection_suggestion(&self, intent_label: &str, count: u32, hash: &str) {
        self.send(&format!(
            "💡 *패턴 발견*: {label}을(를) 자주 하시네요 (최근 {count}회).\n\
             매일 아침 자동으로 실행할까요?\n\
             승인: `kittypaw reflection approve {hash}`\n\
             거절: `kittypaw reflection reject {hash}`",
            label = intent_label,
            count = count,
            hash = hash,
        ));
    }
}

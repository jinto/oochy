# 할 일

- [x] **메모리 레이어 충돌 해결 정책** — 양방향 write-through (save→USER.md, user→DB) + 프롬프트 dedup (Remembered Facts에서 USER.md 키 제외)
- [ ] **Web.search 무설정 기본값 + 폴백 체인** — DuckDuckGo를 기본 백엔드로 (API 키 불필요, 온보딩 변경 없음). Settings 고급 탭에서 SearXNG URL / Brave / Tavily 키 선택적 입력. 유료 API 실패 시 DuckDuckGo 자동 폴백. (리서치: docs/20260409-1500-research-agent-web-search.md)
- [x] **Telegram 메시지 길이 제한 처리** — split_telegram_text() 순수 함수로 4096자 자동 분할. core/engine/channels 3곳 적용.
- [x] **Web.search 쿼리 언어 매칭** — SYSTEM_PROMPT에 "사용자 언어와 같은 언어로 검색 쿼리 생성" 가이드 추가.
- [x] **Telegram send 통합** — send_text_chunked() 단일 게이트웨이로 3곳 통합 완료. engine/channels는 1줄 호출로 축소.
- [ ] **Proactive Memory — 선호도 감지 + 확인 저장** — (A) 명시적 선호도("이렇게 해주면 좋겠어") 감지 → "기억해둘까요?" 확인 후 Memory.user 저장. (B) 암묵적 패턴(3회 이상 같은 요청) → 자동 저장. (C) Compaction 직전 flush_memories (Hermes 패턴) — 시스템 메시지로 "기억할 거 저장해" 강제 호출. 세 가지 하이브리드.
- [ ] **E2E 테스트 가능한 구조** — serve 없이 텔레그램/WS 채널 → agent_loop → skill executor 전체 파이프라인을 통합 테스트로 검증 가능하도록. 현재 통합 테스트는 agent_loop 직접 호출만 가능, serve 경로(WS 프로토콜, 채널 라우팅)는 미검증. 채널을 mock하거나 in-process serve를 테스트에서 직접 띄우는 구조 필요.

# 리서치 링크

- [x] https://wikidocs.net/338204 — Claude Code 아키텍처 분석 → Quick Win 3개(신뢰성)로 반영 완료
- [x] https://news.hada.io/topic?id=28101 — Hermes Agent 경쟁 분석 → MUST-HAVE 3개 식별 (데몬, 사용자 프로필, 커뮤니티 임포트)
- [x] https://news.hada.io/topic?id=27881 — K-Skill 생태계 → 스킬 후보 5개 도출, 큐레이션 15개 완성으로 반영
- [x] https://x.com/coreyganim/status/2039699858760638747 — 동영상 링크만 포함 (추가 분석 불필요)
- [x] https://x.com/TheJumbledSoul/status/2039414846685663311 — OpenClaw→Hermes 전환 사례. 4일 만에 문학 에이전트 구축 (스타일 프로필, 블로그 자동 배포). Hermes의 자율 실행 능력 데모.
- [x] https://x.com/VadimStrizheus/status/2039517746451419335 — Hermes 영상 클리핑 에이전트. YT 링크 → 분석 → 클립 추출 → 자동 포스팅. 멀티채널(Telegram/WhatsApp/Discord) 입력.
- [x] https://x.com/v81093933/status/2039550482813980943 — Short-Trend-Rader: 인기 숏츠 자막 분석 → 트렌드 시나리오 생성. 로컬 CLI, AI 선택적. KittyPaw 스킬로 만들 수 있는 패턴.
- [x] https://github.com/zeroclaw-labs/zeroclaw/blob/master/docs/i18n/ko/README.md — ZeroClaw(OpenClaw 리브랜딩) 경쟁 분석 완료. 같은 Rust, 로컬 퍼스트, 스킬 시스템.
- [x] https://x.com/Voxyz_ai/status/2039107604656300273 — OpenClaw 3.31: 백그라운드 태스크 통합 (`openclaw tasks list`). cron/subagent/ACP 분리 → 통합 추적. KittyPaw 데몬 설계 참고.
- [x] https://x.com/PiChangelog/status/2040141716418601456 — Pi v0.65.0: Session runtime API (`createAgentSessionRuntime`), session_switch 이벤트 제거 → session_start 통합. 아키텍처 단순화 방향.
- [x] https://x.com/outsource_/status/2040279851815276861 — Hermes Agent 웹 대시보드: 세션 목록, 대시보드, 터미널, 작업 관리. 모바일/침대에서 접근. KittyPaw GUI와 유사한 방향.
- [x] https://x.com/WesRoth/status/2040203308464579012 — Hermes Agent v0.7.0: resilience, stealth, long-term context. 프로덕션 신뢰성 강화. KittyPaw도 같은 방향(방금 완료).
- [x] https://x.com/realsigridjin/status/2040273638423966158 — OpenClaw + clawhip + oh-my-codex 조합: 에이전트 관리(robsters), Discord 트리거, Codex 하네스. 멀티 에이전트 오케스트레이션 패턴.
- [x] https://x.com/arcee_ai/status/2040157679453094212 — Arcee × Nous Research: Trinity 모델 + Hermes Agent 통합. 모델+에이전트 번들 트렌드.
- [x] https://x.com/NameLessAiii/status/2039977924212728310 — OpenClaw→Hermes 전환 + 여러 Hermes 에이전트를 추적하는 대시보드 구축. 멀티 에이전트 모니터링 니즈.
- [x] https://x.com/gkisokay/status/2040044476060864598 — @gkisokay의 OpenClaw→Hermes 전환 아티클 (451 likes, 1396 bookmarks). 에이전트 자가 개선 루프 + supervisor agent 패턴. KittyPaw에 적용 가능한 추가 인사이트 3개 도출.

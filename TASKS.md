# KittyPaw Tasks

> "시작은 3분, 성장은 평생." — see [VISION.md](VISION.md)

---

## 🔴 코드 품질 수정 (2026-04-08 리뷰)

> 출처: look01.md(Claude Code 내부 구조 분석) + 전체 코드베이스 직접 독해
> 원칙: 각 커밋 후 `cargo test --workspace` 통과. 공개 API 변경 최소화.

### 즉시 수정 — 버그 (CRITICAL)

- [x] **C-1** `Supervised` 모드 `ResourceKind::File` 하드코딩 제거
  - `skill_executor/mod.rs`: `Telegram|Http|Web|Slack|Discord` → `Network`, 나머지 → `File`

- [x] **C-2** `TEACH_PROMPT` 누락 primitive 11개 추가
  - `teach_loop.rs`: `Env`, `File`, `Git`, `Shell`, `Agent`, `Moa`, `Vision`, `Image`, `Slack`, `Discord`, `Todo` 추가

- [x] **C-3** `assistant.rs` 모순 지시 + 오프라인 시 불필요한 round-trip 제거
  - `{{REGISTRY_ACTIONS}}` 플레이스홀더: registry 비어있으면 search_registry/recommend_skill 액션을 프롬프트에서 제거

- [x] **C-4** `execute_chain_steps` 체인 스텝 오류 전파
  - `execution.rs`: `let _ = execute_skill_calls(...)` → `if let Ok` + 실패 시 `tracing::warn!`

### 단기 수정 — look01.md 직접 교훈

- [x] **H-1** `build_skills_prompt()` 예산 관리
  - `skill_registry.rs`: 4,000바이트 상한 (`SKILL_BUDGET_BYTES`), 초과 시 이름만 표시

- [x] **H-2** 스케줄 경로 `Store::open` 반복 제거
  - `Arc<tokio::sync::Mutex<Store>>` 공유 인스턴스로 해결
  - migration 013: `skill_schedule` 테이블을 Store 마이그레이션으로 흡수
  - `persistence.rs` 함수 → `kittypaw_store::Store` 메서드로 이관

- [x] **H-3** `skill.name` == `skill.id` 확인 — 설계상 동일 (false positive, 수정 불필요)

### 중기 수정 — 아키텍처

> **TDD 원칙**: 각 항목은 실패 테스트 먼저 작성 → 구현 → 통과 순서로 진행.

- [x] **M-1** `CapabilityChecker` rate limit 지속성
  - **TDD**: `schedule_loop_preserves_rate_limit()` 실패 테스트 먼저
  - 스케줄 루프 수명 동안 checker 인스턴스 유지

- [x] **M-2** `AppPaths` 구조체로 CWD 하드코딩 제거
  - **TDD**: `app_paths_derived_from_config()` 실패 테스트 먼저
  - `schedule/mod.rs:58`, `skill.rs:71`: `.kittypaw/*` 경로를 config에서 파생

- [x] **M-3** 메모리 컨텍스트 상한 확인 (look01.md: 292에이전트 교훈)
  - `LIMIT 100` 이미 존재. `MAX_HISTORY_TURNS` 상수로 추출하여 store-engine 연결
  - `debug_assert` + 테스트로 windows 합 ≤ MAX_HISTORY_TURNS 강제

- [x] **M-4** `ResourceKind::Execute` 변형 추가 (Shell/Git/Agent 분리)
  - **TDD**: `shell_requires_execute_permission()` 실패 테스트 먼저
  - `kittypaw-core/src/permission.rs`: `ResourceKind` enum에 `Execute` 추가
  - `skill_executor/mod.rs`: `Shell|Git|Agent|Moa` → `ResourceKind::Execute`
  - Supervised 배치 경로: 채널(토큰 있음) → 자동 허용, Http/Web → Deny, Execute/File → Deny
  - GUI: "Shell 실행 허용?" 다이얼로그 레이블 추가

- [x] **M-5** Http 온보딩 권한 영속화
  - **TDD**: `http_allowed_after_onboarding_grant()` 실패 테스트 먼저
  - 온보딩 시 "웹에 접속을 허용하시나요?" → `AllowPermanent` → Store에 저장
  - `execute_single_call` 배치 경로: Store에서 Http 권한 확인 후 자동 허용
  - **근거:** 현재 Http.post는 Supervised 배치에서 항상 Deny. 온보딩에서 허락받은 경우 스케줄에서 작동해야 함

---

## ISSUE01 수정 — Once Trigger + Due Fix + History Fix + News Quality

> 스펙: `.ina/specs/think-issue01-once-trigger-due-fix-news.md`  
> 플랜: `.claude/plans/issue01-once-trigger-due-fix-news.md`  
> TDD 원칙: 각 항목은 실패 테스트 먼저 → 구현 → 통과 순서

### I-1: 히스토리 재주입 수정 (원인 3) — `compaction.rs`

- [x] **I-1-T** 실패 테스트: `agent_loop_uses_content_not_code` in `compaction.rs`
- [x] **I-1** 구현: `compaction.rs:76` — `turn.code` → `turn.content`
- [x] **I-1-V** `cargo test -p kittypaw-engine -- compaction` 통과

### I-2: Cron Due 버그 수정 (원인 2) — `schedule/cron.rs`

- [x] **I-2-T** 실패 테스트: `new_recurring_skill_not_immediately_due` in `schedule/mod.rs`
- [x] **I-2** 구현: `cron.rs:34` — `now - 24h` → `now`
- [x] **I-2-V** `cargo test -p kittypaw-engine -- schedule` 통과 (16/16)

### I-3: SkillTrigger.run_at 필드 + is_once_due (원인 1 기반)

- [x] **I-3-T** 실패 테스트 4개: `is_once_due_*` + `is_due_includes_once_trigger`
- [x] **I-3** 구현: `skill.rs`에 `run_at` 필드, `cron.rs`에 `is_once_due` + `is_due` 업데이트
- [x] **I-3-V** `cargo test -p kittypaw-core && cargo test -p kittypaw-engine` 통과 (20/20)

### I-4: parse_once_delay 파싱 — `teach_loop.rs`

- [x] **I-4-T** 실패 테스트: `parse_once_delay_valid_formats`, `_minimum_one_minute`, `_invalid_format`
- [x] **I-4** 구현: `parse_once_delay()` + `MAX_DELAY_MINUTES` 상한 (오버플로우 방지)
- [x] **I-4-V** `cargo test -p kittypaw-engine -- teach_loop` 통과 (15/15)

### I-5: Skill.create "once" 케이스 — `skill_executor/skill_mgmt.rs`

- [x] **I-5** 구현: `skill_mgmt.rs`에 "once" 분기 추가
- [x] **I-5-V** `cargo test -p kittypaw-engine` 통과 (79/79)
- [x] **I-5-P** SYSTEM_PROMPT `## When to create a skill` 섹션에 "once" 예시 + 규칙 추가

### I-6: Schedule 루프 once 처리 + 실행 후 삭제

- [x] **I-6** 구현: `schedule/mod.rs` — once 필터 + 실행 후 delete_skill
- [x] **I-6-V** `cargo test --workspace` 통과 (전체 0 failed)

### I-7: SYSTEM_PROMPT 뉴스 품질 규칙 (원인 4)

- [x] **I-7-T** 실패 테스트: `system_prompt_enforces_news_fetch_pipeline`
- [x] **I-7** 구현: `## News & content quality` 섹션 추가 + CORRECT 예시 교체
- [x] **I-7-V** `cargo test -p kittypaw-engine -- agent_loop` 통과 (8/8)

---

## v1 잔여

- [ ] macOS 코드 사이닝 (Apple Developer $99/yr)
- [ ] 파워유저 10명 인터뷰 + 스킬 후보 확정

## v2 잔여

- [ ] 크로스 채널 컨텍스트 (사용자 ID 기반 통합)
- [ ] 온보딩 폴더 접근 제어 + macOS NSOpenPanel
- [ ] 스킬 무결성 검증 (체크섬, 서명)

## v3 잔여

- [ ] teach loop 키워드 분류기 (automation→경량, analysis→중간)
- [ ] 대화 중 자동 모델 교체
- [ ] 자연어 자동화 조합 (기존 스킬 커스터마이즈)

### Web.search 무설정 기본값 + 폴백 체인 ✅
> 플랜: .claude/plans/web-search-fallback-chain.md | 리서치: docs/20260409-1500-research-agent-web-search.md
- [x] Task 1: `parse_ddg_html()` 순수 함수 + 단위 테스트 (http.rs) — DDG HTML fixture로 title/snippet/url 파싱 검증
- [x] Task 2: `ddg_html_search()` 구현 (http.rs) — `html.duckduckgo.com/html/` POST + parse_ddg_html 호출, 기존 `ddg_instant_answer()` 교체
- [x] Task 3: `web_search_dispatch()` 폴백 체인 리팩터 (http.rs) — 설정 기반 디스패치 + DDG 폴백 + 에러 구분 ("결과 없음" vs "백엔드 전부 실패")
- [x] Task 4: Settings UI 검색 백엔드 섹션 (settings.rs + i18n.rs) — 드롭다운(DDG/Brave/Tavily/Exa) + 조건부 API 키 입력
- [x] Task 5: 통합 테스트 + 하위 호환 검증 — 기존 API 키 설정 사용자 동작 유지 확인

### 기타 백로그
- [ ] 스킬 체이닝 병렬 실행 (`parallel()`)
- [ ] AI 비서 프리셋 (캐릭터 + 말투 + 배경지식)
- [ ] /daily 모닝 브리핑 (Todoist + Calendar)
- [ ] LLM 기반 파라미터 자동 조정
- [ ] LLM 요약 (세션간 기억)

---

## Wow #2: 학습하는 엔진 (Adaptive Engine) ← 현재

Spec: `.ina/specs/20260409-think-adaptive-engine.md`
Plan: `.claude/plans/adaptive-engine.md`

### Plan 1: Store + Config 기반 ✅
- [x] ReflectionConfig 추가 + recent_user_messages 쿼리
- [x] reflection:* 키 격리 + "Learned Patterns" 섹션

### Plan 2: Reflection Loop 핵심 ✅
- [x] reflection.rs — LLM intent grouping + suggestion 생성
- [x] schedule/mod.rs — reflection tick 통합

### Plan 3: CLI + 승인 흐름 ✅
- [x] kittypaw reflection list/approve/reject/clear CLI

---

## KakaoTalk 채널 (Open Builder + CF Worker Relay) ✅

> 스펙: `.ina/specs/20260409-0220-think-kakao-channel.md`
> 플랜: `.claude/plans/kakao-channel-open-builder-relay.md`
> TDD 원칙: 실패 테스트 먼저 → 구현 → 통과

### Plan 1: KittyPaw Rust 채널 구현

- [x] **K-1-T** 실패 테스트: `kakao_event_session_id`, `kakao_channel_name` (types.rs)
- [x] **K-1** 구현: `EventType::KakaoTalk` + `session_id()` + `channel_name()` + `KakaoChannelConfig`
- [x] **K-2-T** 실패 테스트: `kakao_channel_name_is_kakao`, `kakao_parses_openbuilder_payload` (MockRelayClient)
- [x] **K-2** 구현: `kakao.rs` 신규 — `KakaoChannel` + `RelayClient` trait + `Channel` impl
- [x] **K-3** 구현: `lib.rs` + `registry.rs` + `skill_executor/mod.rs` match arm 추가
- [x] **K-4** 구현: `prompt.rs` (format_event) + `serve.rs` KakaoTalk 응답 라우팅
- [x] **K-5** 검증: `cargo test --workspace` 통과

### Plan 2: CF Worker Relay ← Plan 1 완료 후 진행

- [x] **R-1** relay/ 초기화 (wrangler + vitest-pool-workers + TypeScript)
- [x] **R-2-T** 실패 테스트: HMAC 검증 + KV 저장 (miniflare)
- [x] **R-2** 구현: `POST /webhook` — HMAC 검증 → useCallback 반환 → KV 저장
- [x] **R-3-T** 실패 테스트: atomic poll (fetch+delete)
- [x] **R-3** 구현: `POST /poll/{user_token}` — atomic fetch+delete, 없으면 204

---

## 테스트 격리 + LLM Eval Framework ← 현재

> 플랜: `.claude/plans/generic-hugging-harbor.md`  
> 스펙: `.ina/specs/20260409-1800-think-llm-eval-reflection.md`

### Plan A: KITTYPAW_HOME 격리 (전제조건)

- [x] **A-1** `helpers.rs`: `db_path()` 기본값 → `data_dir().join("kittypaw.db")`
- [x] **A-2** `e2e-reflection.sh`: KITTYPAW_HOME 격리 + trap cleanup
- [x] **A-3** `e2e-schedule.sh`: KITTYPAW_HOME 격리 + trap cleanup

### Plan B: LLM Eval Framework

- [x] **B-1** `kittypaw-engine/Cargo.toml`: `[features] llm-eval = []` 추가
- [x] **B-2** `reflection.rs`: `#[cfg(feature = "llm-eval")]` eval 모듈 (골든셋 2개 + behavioral invariant 1개)

---

## 참고

- [VISION.md](VISION.md) — 철학, 포지셔닝, 마일스톤
- look01.md — Claude Code 내부 구조 분석 (Agent/Team/Skill 시스템)

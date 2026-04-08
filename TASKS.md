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

- [ ] **H-2** 스케줄 경로 `Store::open` 반복 제거 (보류)
  - `rusqlite::Connection`의 `Send` 여부 + sandbox thread model 확인 필요

- [x] **H-3** `skill.name` == `skill.id` 확인 — 설계상 동일 (false positive, 수정 불필요)

### 중기 수정 — 아키텍처

> **TDD 원칙**: 각 항목은 실패 테스트 먼저 작성 → 구현 → 통과 순서로 진행.

- [x] **M-1** `CapabilityChecker` rate limit 지속성
  - **TDD**: `schedule_loop_preserves_rate_limit()` 실패 테스트 먼저
  - 스케줄 루프 수명 동안 checker 인스턴스 유지

- [ ] **M-2** `AppPaths` 구조체로 CWD 하드코딩 제거
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

- [ ] **M-5** Http 온보딩 권한 영속화
  - **TDD**: `http_allowed_after_onboarding_grant()` 실패 테스트 먼저
  - 온보딩 시 "웹에 접속을 허용하시나요?" → `AllowPermanent` → Store에 저장
  - `execute_single_call` 배치 경로: Store에서 Http 권한 확인 후 자동 허용
  - **근거:** 현재 Http.post는 Supervised 배치에서 항상 Deny. 온보딩에서 허락받은 경우 스케줄에서 작동해야 함

---

## v1 잔여

- [ ] macOS 코드 사이닝 (Apple Developer $99/yr)
- [ ] 파워유저 10명 인터뷰 + 스킬 후보 확정

## v2 잔여

- [ ] 카카오톡 연동
- [ ] 크로스 채널 컨텍스트 (사용자 ID 기반 통합)
- [ ] 온보딩 폴더 접근 제어 + macOS NSOpenPanel
- [ ] 스킬 무결성 검증 (체크섬, 서명)

## v3 잔여

- [ ] teach loop 키워드 분류기 (automation→경량, analysis→중간)
- [ ] 대화 중 자동 모델 교체
- [ ] 자연어 자동화 조합 (기존 스킬 커스터마이즈)
- [ ] 웹 검색 프로바이더 폴백 체인 (Exa → DuckDuckGo)
- [ ] 스킬 체이닝 병렬 실행 (`parallel()`)
- [ ] AI 비서 프리셋 (캐릭터 + 말투 + 배경지식)
- [ ] /daily 모닝 브리핑 (Todoist + Calendar)
- [ ] LLM 기반 파라미터 자동 조정
- [ ] LLM 요약 (세션간 기억)

---

## 참고

- [VISION.md](VISION.md) — 철학, 포지셔닝, 마일스톤
- look01.md — Claude Code 내부 구조 분석 (Agent/Team/Skill 시스템)

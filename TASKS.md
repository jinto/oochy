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

- [x] **C-3** `assistant.rs` 모순 지시 수정
  - "Do NOT use search_registry" → search_registry 활용 권장으로 교체

- [x] **C-4** `execute_chain_steps` 체인 스텝 오류 전파
  - `execution.rs`: `let _ = execute_skill_calls(...)` → `if let Ok` + 실패 시 `tracing::warn!`

### 단기 수정 — look01.md 직접 교훈

- [x] **H-1** `build_skills_prompt()` 예산 관리
  - `skill_registry.rs`: 4,000바이트 상한 (`SKILL_BUDGET_BYTES`), 초과 시 이름만 표시

- [ ] **H-2** 스케줄 경로 `Store::open` 반복 제거 (보류)
  - `rusqlite::Connection`의 `Send` 여부 + sandbox thread model 확인 필요

- [x] **H-3** `skill.name` == `skill.id` 확인 — 설계상 동일 (false positive, 수정 불필요)

### 중기 수정 — 아키텍처

- [ ] **M-1** `CapabilityChecker` rate limit 지속성
  - 스케줄 루프 수명 동안 checker 인스턴스 유지

- [ ] **M-2** `AppPaths` 구조체로 CWD 하드코딩 제거
  - `schedule/mod.rs:58`, `skill.rs:71`: `.kittypaw/*` 경로를 config에서 파생

- [ ] **M-3** 메모리 컨텍스트 상한 확인 (look01.md: 292에이전트 교훈)
  - `compaction.rs` `compact_turns()` 상한 파악 및 캡 설정

- [ ] **M-4** `ResourceKind::Execute` 변형 추가 (Shell/Git/Agent 분리)
  - `kittypaw-core/src/permission.rs`: `ResourceKind` enum에 `Execute` 추가
  - `skill_executor/mod.rs`: `Shell|Git|Agent|Moa` → `ResourceKind::Execute`
  - GUI: "Shell 실행 허용?" 다이얼로그 레이블 추가
  - **근거:** 현재 Shell/Git은 `File`로 분류되어 사용자가 "파일 접근" 허용 시 Shell 명령도 실행됨

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

# KittyPaw Tasks

> "시작은 3분, 성장은 평생." — see [VISION.md](VISION.md)

## Completed

### Skill Platform — Phase 0~4 ✅
- [x] Phase 0: SkillResolver (샌드박스 실제 데이터 반환)
- [x] Phase 1: 패키지 포맷 + 매니저 + executor 브릿지
- [x] Phase 2: File.read/write, Telegram.sendDocument, Env.get
- [x] Phase 3: GUI 스킬 갤러리 + 설정 위자드 (Dioxus)
- [x] Phase 4: 예제 패키지 5개 (한국어) + 자동 번들 설치

### GUI: Tauri → Dioxus 전환 ✅
- [x] Tauri + SvelteKit 삭제 (~24k LOC)
- [x] Dioxus 0.6 순수 Rust GUI (~470 LOC)
- [x] GUI 채팅 → 실제 LLM 호출 (ClaudeProvider)
- [x] 스킬 Test Run 버튼 (SkillResolver 연동)

### Foundation 기반 기능 4개 ✅
- [x] 로컬 시크릿 저장소 (`~/.kittypaw/secrets.json`, atomic write)
- [x] 멀티 프로바이더 LLM (OpenAI + Claude + LlmRegistry)
- [x] Web.search / Web.fetch 샌드박스 프리미티브
- [x] 스킬 체이닝 (`[[chain]]` + prev_output 전달)

### 로컬 LLM 지원 (Ollama/llama.cpp) ✅
- [x] `OpenAiProvider`에 `base_url` 파라미터 추가 (Ollama 호환)
- [x] `kittypaw.toml` `[[models]]`에 `base_url` 필드 지원
- [x] GUI Settings에 로컬 모델 연결 UI (URL + 모델명 입력)
- [x] CLI에서 `LlmRegistry::from_configs()` 연결
- [x] `base_url` 보안 검증 (SSRF 방어 + API key 유출 방지)
- [x] Config keychain fallback (TOML → env → keychain 통합)

### 문서 + 마케팅 ✅
- [x] README 리뉴얼 (Use Case 중심, 한국어)
- [x] kittypaw.app 랜딩 페이지 (Cozy Tech 테마)
- [x] kittypaw-skills GitHub org 생성
- [x] SEO 최적화 + 영문/일문 랜딩 페이지 (i18n, hreflang, JSON-LD)

### 보안 수정 ✅
- [x] Web.fetch SSRF 리다이렉트 차단
- [x] UTF-8 멀티바이트 truncation 패닉 수정
- [x] 체인 스텝 skill calls 실행 누락 수정

---

## v1: Silent Engine

> 철학: "최고의 AI는 보이지 않는 AI다"
> 검증 목표: "5분 안에 설치하고, 1주 후 AI가 있다는 걸 잊었는가"
> 성공 기준: 파워유저 10명 중 7명이 5분 내 첫 실행, 과반이 "AI를 의식하지 않았다"

### 🔴 P0: 사용자 리서치 (코드 전에)
> 디자인 문서 Assignment: "코드 한 줄 짜지 말고 답변 10개를 모아라"
- [ ] "기술 인접 파워유저" 10명 찾기 (주변, 온라인 커뮤니티)
- [ ] 인터뷰 질문 1: "매일 반복하는 작업 중 AI가 대신 해줬으면 하는 게 뭐야?"
- [ ] 인터뷰 질문 2: "그 자동화가 돌아갈 때, AI가 보이는 게 좋아? 결과만 나오는 게 좋아?"
- [ ] 답변 정리 → 큐레이션 스킬 후보 10개 확정 + 철학 검증

### 🔴 P0: 스킬 스토어 구현 ✅
> 기존 로컬 갤러리를 리모트 레지스트리 기반 스토어로 확장
- [x] `kittypaw-skills/registry` 레포 + index.json 스키마 설계
- [x] 앱에서 registry HTTP fetch + 캐싱
- [x] 스킬 스토어 브라우즈 UI (리모트 스킬 목록 표시)
- [x] 원클릭 설치 플로우 (다운로드 → 설치 → 완료 알림)
- [x] 에러 핸들링 (네트워크 실패, 호환성, 오프라인 시 캐시 표시)
- [x] 보안: SSRF 방어 (URL 화이트리스트), path traversal 차단 (ID 검증), 패키지 ID 일치 확인

### 🔴 P0: Silent Memory ✅
> "같은 엔진, 다른 차" — Hermes의 기술을 KittyPaw 철학으로 쓴다
- [x] kittypaw-store migration 005: execution_history + user_context 테이블
- [x] schedule.rs `run_skill()` 후 실행 기록 삽입
- [x] 대시보드 실데이터 연결 (mock → DB 쿼리)
- [x] 프라이버시: 결과 500자 제한, 30일 자동 삭제
- [x] 패턴 매칭: 같은 파라미터 3회 → 기본값 자동 적용
- [x] 패턴 매칭: 실패 시 자동 재시도 (exponential backoff)
- [x] 패턴 매칭: 시간 패턴 감지 → 스케줄 제안 (대시보드 제안 카드)

### 🔴 P0: GUI 대시보드 퍼스트 ✅
> 메인 화면은 "내 자동화 현황판" — 채팅이 아니다
- [x] `dashboard.rs` 신규 컴포넌트 (활성 스킬 + 실행 결과 + 다음 스케줄)
- [x] 오늘의 실행 요약 (성공/실패/자동최적화)
- [x] "조용한 개선" 카운터: "이번 주 N번의 자동 최적화 적용됨"
- [x] 채팅을 세 번째 탭 또는 사이드 패널로 이동 (설정/디버깅 전용)
- [x] 앱 실행 시 대시보드가 기본 화면

### 🔴 P0: 큐레이션 스킬 10개
> 기존 5개 + 사용자 리서치 기반 5개
- [x] weather-briefing (아침 날씨 요약)
- [x] url-monitor (페이지 변경 감지)
- [x] rss-digest (RSS 피드 요약)
- [x] macro-economy-report (거시경제 리포트)
- [x] reminder (리마인더)
- [x] transit-arrival (지하철/버스 도착정보)
- [x] kbo-scores (KBO 프로야구 결과)
- [x] price-watch (가격 알림)
- [x] zipcode-lookup (우편번호 검색)
- [x] exchange-rate (환율 알림)

### 🟠 P1: 배포 파이프라인
- [x] kittypaw.app 도메인 DNS 설정 (Cloudflare → GitHub Pages)
- [x] GitHub Actions 릴리즈 CI 재작성 (kittypaw-gui `.app` 번들 + `.dmg` + CLI 바이너리)
- [ ] macOS 코드 사이닝 검토 (Apple Developer $99/yr, Gatekeeper 마찰 감소)

### 🟠 P1: 온보딩 UX ✅
> v1 타겟: "코딩 인터페이스는 싫어하는 기술 인접 파워유저"
- [x] GUI 온보딩 위자드 (3단계: 환영 → LLM 선택 → 완료)
- [x] LLM API 키 온보딩: 로컬 LLM(Ollama) or Claude API 선택
- [x] main.rs 부트스트랩: keychain에서 LLM 설정 로드 (두 번째 실행부터 자동)

---

## v2: Deeper Silence (v1 검증 후)

### 스킬 간 컨텍스트 공유 ✅
- [x] user_context를 모든 스킬이 읽을 수 있게 (ctx.user.location 등)
- [x] 자동 스킬 제안 (v1 Silent Memory Phase 2에서 detect_time_pattern으로 구현됨)

### 보이지 않는 자기 개선 ✅
- [x] 실패 힌트 저장 + 성공 시 자동 클리어 (failure_hint user_context)
- [x] 실행 로그 기록 (`~/.kittypaw/execution.jsonl`)
- [ ] LLM 기반 파라미터 자동 조정 (v3로 이동 — LLM 호출 인프라 필요)

### 세션간 기억 ✅
- [x] FTS5 전문 검색 (execution_fts 가상 테이블 + search_executions)
- [ ] LLM 요약 (v3로 이동)
- [ ] 사용 패턴 기반 스킬 추천 (v1에서 detect_time_pattern으로 일부 구현)

### 멀티채널 — 결과 알림으로
- [x] Slack 채널 어댑터 (Channel trait + skill executor + Settings UI)
- [x] Discord 채널 어댑터 (Channel trait + skill executor + Settings UI)
- [ ] 카카오톡 연동
- [ ] 크로스 채널 컨텍스트 (사용자 ID 기반 통합)

### 커뮤니티 스킬 마켓플레이스
- [x] kittypaw-skills/registry GitHub 레포 생성 + index.json + 5개 스킬
- [x] 유저 제작 스킬: Fork → packages/ 추가 → PR로 공유
- [ ] 스킬 무결성 검증 (체크섬, 서명) — v3로 이동

---

## v3: Invisible Infrastructure (v2 안정화 후)

### 자연어 자동화 조합
- [ ] 기존 스킬을 자연어로 커스터마이즈
- [ ] AI가 스킬 조합/수정해서 새 자동화 생성

### 모델 자동 라우팅 (사용자는 모름)
- [x] `package.toml`에 `model` 필드 → 스킬별 모델 지정
- [x] skill_config UI에 모델 선택 드롭다운
- [x] execute_skill_calls에 model_override 전달 (체인 스텝 포함)
- [ ] teach loop 키워드 분류기 (automation→경량, analysis→중간)
- [ ] 대화 중 자동 모델 교체

### 스킬 설정 위자드 (pi-wizard 영감)
- [x] skill_config.rs를 단계별 위자드로 — 필드를 한 번에 하나씩, 안내 텍스트 포함
- [x] 스킬별 setup guide (예: "텔레그램에서 @BotFather → /newbot → 토큰 복사")

### Extension 시스템 (v3+, 검토 필요)
- [ ] 스킬이 커스텀 UI를 그리거나 도구를 등록할 수 있는 확장 레이어
- [ ] agentskills.io 호환 검토 (Pi/Claude Code/Codex CLI 공용 스킬 포맷)

### Agent Runtime Hardening (Claude Code 분석 참고)
- [x] `agent_loop.rs` 상태 전이 명시화 (`generate` → `execute` → `retry` → `finish`) + transition reason 로그
- [x] 단계별 컨텍스트 압축: 최근 턴 유지 + tool/output 축약 + 오래된 대화 요약
- [x] sandbox file/network permission popup wiring 완료 (`AskUser`, `AllowOnce`, `AllowPermanent`)
- [x] 기능 플래그 / kill switch 레이어 (background agents, model routing, experimental channels)
- [x] 실패 복구 정책 고도화: token budget 초과 시 compact 후 재시도, 프롬프트 축소, 경량 모델 fallback
- [x] 권한 우회 경로 제거: `resolve_skill_call`에 permission callback 전달, headless auto-allow 차단
- [x] 스킬 입력 검증 일관성: Telegram sendMessage/sendPhoto/sendDocument + Storage.set/delete 빈 인자 조기 차단
- [x] TransitionReason 실사용: agent_loop 전 단계에 구조화된 transition reason 로그 연결
- [x] 스킬 결과 크기 제한: resolve_skill_call에서 50KB 초과 시 유효한 JSON 에러 반환 (Tool Result Budget)
- [x] 토큰 추정 기반 컨텍스트 예산: estimate_tokens() + 프로액티브 TOKEN_BUDGET 체크 (LLM 호출 전 컴팩션)
- [x] 스킬 에러 분류 + 단위 재시도: KittypawError::is_transient() + Http/Web Skill 에러 1회 재시도
- [x] 전체 에이전트 루프 타임아웃: main.rs에서 tokio::time::timeout으로 sandbox_timeout × 4 적용
- [x] Circuit Breaker: TokenLimit at max compaction(attempt≥2) → 즉시 break, LLM API 낭비 차단
- [x] Safe/Unsafe 스킬 병렬화 분류: is_read_only_skill_call() + 읽기 전용 배치 감지 로그 (병렬 실행 인프라)
- [x] LLM 네트워크 에러 복원력: LlmErrorKind::Network + 413→TokenLimit + reqwest 에러 분류 + agent_loop 재시도
- [x] 동적 토큰 예산: LlmProvider::context_window() + max_tokens() trait + ConfiguredProvider config override
- [x] Fallback 모델 자동 전환: LlmRegistry::fallback_provider() (insertion order 보장) + transient 에러 소진 후 전환
- [x] LLM 프로바이더 코드 정리: classify_reqwest_error + handle_response_status 공유 헬퍼 + AgentLoopParams 구조체
- [x] LLM 토큰 사용량 추적: LlmResponse + TokenUsage 타입, Claude/OpenAI usage 파싱, usage_json DB 저장, 대시보드 표시
- [x] CLI status/log 명령: `kittypaw status` (오늘 통계) + `kittypaw log` (실행 이력) + 토큰 예산 표시
- [x] 일일 토큰 한도: `daily_token_limit` FeatureFlag + agent_loop 사전 체크

### 기타 백로그
- [ ] 웹 검색 프로바이더 폴백 체인 (Exa → DuckDuckGo)
- [ ] 스킬 체이닝 병렬 실행 (`parallel()`)
- [ ] AI 비서 프리셋 (캐릭터 + 말투 + 배경지식)
- [ ] 자율 최적화 루프 (`kittypaw optimize`)
- [x] 한국 특화 스킬 5개 (미세먼지, 배송조회, 로또, 뉴스요약, 주식알림)
- [x] 콘텐츠 파이프라인: trend-scanner → content-drafter 체인 (첫 [[chain]] 활용 사례)
- [x] 자율성 레벨: AutonomyLevel (readonly/supervised/full) + execute_single_call 게이트
- [x] 디바이스 페어링: paired_chat_ids + /pair 명령
- [x] 자가 개선 루프: 스킬 2회 실패 시 LLM 자동 수정 (AutonomyLevel 연동, Full 모드)
- [x] agentskills.io SKILL.md 네이티브 지원: 파서 + 로더 + LLM 프롬프트 주입 실행 + .agents/skills/ 스캔
- [ ] /daily 모닝 브리핑 (Todoist + Calendar)

---

## 경쟁 포지셔닝

| | GUI | 로컬LLM | 스킬스토어 | AI 투명성 | 대시보드 | 오픈소스 |
|---|---|---|---|---|---|---|
| Hermes Agent | ❌ | ✅ | ✅ | ❌ (AI 주인공) | ❌ | ✅ |
| OpenClaw | ❌ | ❌ | ✅ (13k+) | ❌ | ❌ | ✅ |
| Pi | ❌ | ❌ | ❌ | ❌ | ❌ | ✅ |
| Atomic Bot | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Manus Desktop | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| Thoth | ✅ | ✅ | ❌ | ❌ | ✅ | ✅ |
| **KittyPaw** | **✅** | **✅** | **✅** | **✅ (AI 사라짐)** | **✅** | **✅** |

## 참고 자료

- [VISION.md](VISION.md) — 철학, 포지셔닝, 마일스톤
- [디자인 문서](~/.gstack/projects/jinto-kittypaw/jinto-main-design-20260330-154436.md) — "최고의 AI는 보이지 않는 AI다" 전체 분석
- [Hermes Agent](https://hermes-agent.nousresearch.com/) — "에이전트가 너와 함께 자란다", NousResearch
- [OpenClaw](https://openclaw.ai/) — NVIDIA 후원, 25만+ 스타
- [Pi](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/) — 미니멀 에이전트 (Mario Zechner)
- [Atomic Bot](https://atomicbot.ai/) — OpenClaw 원클릭 데스크톱
- [Thoth](https://github.com/siddsachar/Thoth) — 로컬 AI 어시스턴트, 네이티브 GUI

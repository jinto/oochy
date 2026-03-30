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
- [x] OS keychain 시크릿 관리 (`keyring` crate)
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
- [ ] 사용자 리서치 결과에 따라 5개 추가

### 🟠 P1: 배포 파이프라인
- [ ] kittypaw.app 도메인 DNS 설정 (Cloudflare → GitHub Pages)
- [ ] GitHub Actions 릴리즈 CI 재작성 (현재 `oochy` CLI 전용 → Dioxus `.app` 번들 + `.dmg` 패키징)
- [ ] macOS 코드 사이닝 검토 (Apple Developer $99/yr, Gatekeeper 마찰 감소)

### 🟠 P1: 온보딩 UX
> v1 타겟: "코딩 인터페이스는 싫어하는 기술 인접 파워유저"
- [ ] GUI 온보딩 위자드 (API 키 입력 or 로컬 LLM 선택, kittypaw.toml 수동 편집 불필요)
- [ ] LLM API 키 온보딩 마찰 해결 (로컬 LLM 기본값? 가이드 위자드? 무료 티어?)

---

## v2: Deeper Silence (v1 검증 후)

### 스킬 간 컨텍스트 공유
- [ ] user_context를 모든 스킬이 읽을 수 있게 (위치 → 날씨+출퇴근 공유)
- [ ] 자동 스킬 제안 (실행 패턴 기반)

### 보이지 않는 자기 개선
- [ ] 실패한 스킬 자동 수정 (LLM 기반 파라미터 조정 — v1에서 제외한 것)
- [ ] 성공한 스킬 최적화 (GEPA 방식, 사용자에게 보이지 않게)
- [ ] 실행 로그 기록 (`execution.jsonl`)

### 세션간 기억
- [ ] FTS5 전문 검색 + LLM 요약
- [ ] 사용 패턴 기반 스킬 추천

### 멀티채널 — 결과 알림으로
- [ ] Slack 채널 어댑터 (결과 알림)
- [ ] Discord 채널 어댑터 (결과 알림)
- [ ] 카카오톡 연동
- [ ] 크로스 채널 컨텍스트 (사용자 ID 기반 통합)

### 커뮤니티 스킬 마켓플레이스
- [ ] 유저 제작 스킬 업로드/공유
- [ ] 스킬 무결성 검증 (체크섬, 서명)

---

## v3: Invisible Infrastructure (v2 안정화 후)

### 자연어 자동화 조합
- [ ] 기존 스킬을 자연어로 커스터마이즈
- [ ] AI가 스킬 조합/수정해서 새 자동화 생성

### 모델 자동 라우팅 (사용자는 모름)
- [ ] teach loop 키워드 분류기 (automation→경량, analysis→중간)
- [ ] 대화 중 자동 모델 교체
- [ ] `package.toml`에 `model` 필드 → 스킬별 모델 지정

### 기타 백로그
- [ ] 웹 검색 프로바이더 폴백 체인 (Exa → DuckDuckGo)
- [ ] 스킬 체이닝 병렬 실행 (`parallel()`)
- [ ] AI 비서 프리셋 (캐릭터 + 말투 + 배경지식)
- [ ] 자율 최적화 루프 (`kittypaw optimize`)
- [ ] 한국 특화 스킬 패키지 (SRT, 미세먼지, KBO)
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

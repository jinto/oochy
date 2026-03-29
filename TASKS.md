# KittyPaw Tasks

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

### 문서 + 마케팅 ✅
- [x] README 리뉴얼 (Use Case 중심, 한국어)
- [x] kittypaw.app 랜딩 페이지 (Cozy Tech 테마)
- [x] kittypaw-skills GitHub org 생성

### 보안 수정 ✅
- [x] Web.fetch SSRF 리다이렉트 차단
- [x] UTF-8 멀티바이트 truncation 패닉 수정
- [x] 체인 스텝 skill calls 실행 누락 수정

---

## In Progress

### 🔴 P0: 로컬 LLM 지원 (Ollama/llama.cpp)
> Hermes Agent 급성장 근거. "토큰 먹는 하마" 불만 해소. 구현 쉬움.
- [ ] `OpenAiProvider`에 `base_url` 파라미터 추가 (Ollama 호환)
- [ ] `kittypaw.toml` `[[models]]`에 `base_url` 필드 지원
- [ ] GUI Settings에 로컬 모델 연결 UI (URL + 모델명 입력)
- [ ] 로컬 모델용 예제 config (`ollama`, `lm-studio` 등)
- [ ] 랜딩 페이지에 "로컬 LLM 지원" 뱃지 + 경쟁 비교표 추가

### 배포 준비
- [ ] kittypaw.app 도메인 DNS 설정 (Cloudflare → GitHub Pages)
- [ ] `kittypaw-skills/registry` 레포 + index.json
- [ ] 앱에서 registry fetch → 갤러리 표시 → 원클릭 설치

---

## Backlog

### 🔴 P0: 스킬 자동 개선 (재귀적 수정)
> Crew의 킬러 기능. "AI가 이미 PR 보내놓은 뒤"
- [ ] 스킬 실행 실패 → 에러 로그를 LLM에 전달
- [ ] LLM이 코드 수정 → 재실행 (최대 3회)
- [ ] 성공 시 수정된 코드 자동 저장
- [ ] 실행 로그 기록 (`execution.jsonl`)
- **마케팅**: "쓸수록 똑똑해지는 자동화"

### 🟠 P1: 모델 자동 라우팅
> Crew 패턴. 가성비 스윗스팟.
- [ ] `kittypaw.toml` `[[models]]` → LlmRegistry 자동 등록
- [ ] teach loop 키워드 분류기 (automation→경량, analysis→중간, integration→고급)
- [ ] 대화 중 자동 모델 교체 (사용자 모르게)
- [ ] 2단계 신뢰도 게이팅 (high=자동, medium=추천)
- [ ] `package.toml`에 `model` 필드 → 스킬별 모델 지정

### 🟠 P1: 웹 검색 개선
- [ ] 검색 프로바이더 폴백 체인 (Exa → DuckDuckGo → 커스텀)
- [ ] Web.fetch 마크다운 추출 개선 (Readability 패턴)
- [ ] GUI 검색 큐레이션 (결과 선택 → 스킬 주입)

### 🟡 P2: 추가 채널 어댑터
> Crew가 7개 메신저 지원. KittyPaw는 현재 Telegram + GUI만.
- [ ] Slack 채널 어댑터
- [ ] Discord 채널 어댑터
- [ ] 크로스 채널 컨텍스트 (사용자 ID 기반 통합)
- [ ] 카카오톡 연동 (k-skill 참고)

### 🟡 P2: 스킬 체이닝 확장
- [ ] 병렬 실행 (`parallel()`)
- [ ] `converge` 모드 (변경 없으면 조기 종료)
- [ ] 체인 단계별 모델 로테이션

### 🟡 P2: AI 비서 프리셋
> ginipigi 아티클 패턴
- [ ] 지침 템플릿 시스템 (캐릭터 + 말투 + 배경지식)
- [ ] 팩트체크 파이프라인 (복수 LLM 교차검증)
- [ ] 콘텐츠 회고 스킬 (데이터 → 패턴 분석)

### 🟢 P3: 자율 최적화 루프
> pi-autoresearch 패턴
- [ ] `kittypaw optimize <skill> --metric <name>`
- [ ] 최적화 루프: 코드 수정 → 벤치마크 → 유지/리버트
- [ ] 신뢰도 점수 (MAD 기반)
- [ ] `optimization.jsonl` + `optimization.md` (세션 재개)

### 🟢 P3: 한국 특화 스킬 패키지
> k-skill 참고
- [ ] SRT/KTX 예약, 배송 조회, 미세먼지
- [ ] 로또, KBO, 환율 알림
- [ ] sops + age 시크릿 관리

### 🟢 P3: /daily 모닝 브리핑
- [ ] Todoist + Obsidian Tasks 통합
- [ ] Google Calendar 미팅 조회
- [ ] 데모 (VHS GIF)

---

## 경쟁 포지셔닝

```
                  GUI    로컬LLM   스킬갤러리   자동스킬생성   오픈소스   무료
Hermes Agent       ❌      ✅        ❌          ✅          ✅       ✅
OpenClaw           ❌      ❌        ❌          ❌          ✅       ✅
Crew               ✅      ❌        ❌          ❌          ❌       ❌
KittyPaw           ✅      🔜        ✅          ✅          ✅       ✅
```

## 참고 자료

- [Hermes Agent](https://hermes-agent.nousresearch.com/) — 로컬 LLM 에이전트, Nous Research
- [Crew](https://crew.day) — 7주만에 혼자 출시한 AI 비서 SaaS
- [pi-autoresearch](https://github.com/davebcn87/pi-autoresearch) — 자율 최적화 루프
- [k-skill](https://github.com/NomaDamas/k-skill) — 한국 특화 스킬 컬렉션
- [model-router](https://github.com/umgbhalla/pi-config) — 모델 자동 라우팅
- `docs/research-2026-03-28.md` — PI 생태계 종합 분석

# KittyPaw 🐾

**코드 없이 자동화하는 AI 데스크톱 앱**

스킬 갤러리에서 원하는 자동화를 설치하고, 설정만 채우면 끝.
매일 아침 시장 리포트, RSS 뉴스 요약, URL 모니터링... 전부 자동으로.

```bash
cargo run -p kittypaw-gui
```

> 순수 Rust GUI. npm 없음. JS 없음. 단일 바이너리.

---

## Use Cases

### 📈 매일 아침 시장 리포트

PM이 출근 전에 글로벌 시장 요약을 받고 싶다면:

1. Skills 탭 → **매크로 경제 리포트** 클릭
2. 텔레그램 봇 토큰 + 채팅 ID 입력
3. 스케줄: `0 8 * * 1-5` (평일 오전 8시)
4. 끝. 매일 아침 ETF 데이터 + AI 요약이 텔레그램으로 옵니다.

### 📰 RSS 뉴스 요약

Hacker News, TechCrunch 등 RSS 피드의 새 글을 AI가 요약해서 보내줍니다:

1. Skills 탭 → **RSS 뉴스 요약** 설치
2. RSS 피드 URL 입력 (기본: Hacker News)
3. 이미 본 글은 자동 필터링, 새 글만 요약

### 🔍 URL 모니터링

내 서비스가 다운되면 즉시 알림:

1. Skills 탭 → **URL 모니터** 설치
2. 모니터링할 URL + 텔레그램 설정
3. 5분마다 체크, 상태 변경 시 알림

### 🌤 날씨 브리핑

매일 아침 내 도시의 7일 예보를 자연어로:

1. Skills 탭 → **날씨 브리핑** 설치
2. 도시 이름 + 좌표 입력
3. API 키 불필요 (Open-Meteo 무료 API)

### ✅ 리마인더

채팅으로 할 일 관리:

```
"remind 우유 사기"     → 할 일 추가
"remind 목록"          → 전체 보기
"remind 완료 1"        → 1번 완료 처리
```

### 💬 AI 채팅

Settings에서 API 키 입력 후, 채팅 탭에서 바로 대화:

- Claude Sonnet 기반 AI 어시스턴트
- 대화 기록 유지
- 코드 없이 자연어로 질문

---

## 스킬 갤러리

앱을 열면 5개 예제 스킬이 바로 설치되어 있습니다:

| 스킬 | 설명 | 트리거 |
|------|------|--------|
| 매크로 경제 리포트 | ETF 데이터 + AI 요약 → 텔레그램 | 스케줄 (평일 8시) |
| 날씨 브리핑 | 7일 예보 → 자연어 브리핑 → 텔레그램 | 스케줄 (매일 7시) |
| RSS 뉴스 요약 | RSS 새 글 → AI 요약 → 텔레그램 | 스케줄 (매일 9시) |
| 리마인더 | 채팅으로 할 일 관리 | 키워드 ("remind") |
| URL 모니터 | 상태 변경 감지 → 알림 | 스케줄 (5분마다) |

### 나만의 스킬 만들기

CLI에서 자연어로 스킬을 만들 수 있습니다:

```bash
kittypaw teach "매일 아침 로또 당첨번호 확인해서 텔레그램으로 보내줘"
```

AI가 코드를 생성하고, 테스트 실행 후, 승인하면 자동 등록됩니다.

---

## 설치

### 빌드

```bash
git clone https://github.com/jinto/kittypaw.git
cd kittypaw
cargo build --release
```

### GUI 실행

```bash
cargo run -p kittypaw-gui
```

### CLI 실행

```bash
# 설정 초기화
cargo run -p kittypaw-cli -- init

# 스킬 생성
cargo run -p kittypaw-cli -- teach "설명"

# 봇 서버 시작 (텔레그램 + 스케줄러)
cargo run -p kittypaw-cli -- serve
```

---

## 기술 스택

순수 Rust. 외부 런타임(Node.js, Python) 없음.

| 구성 요소 | 기술 |
|----------|------|
| GUI | [Dioxus](https://dioxuslabs.com/) 0.6 (데스크톱) |
| 샌드박스 | QuickJS VM + macOS Seatbelt / Linux Landlock |
| LLM | Claude API + OpenAI API + 로컬 LLM (Ollama/LM Studio/llama.cpp) |
| 저장소 | SQLite (rusqlite) + 로컬 시크릿 저장소 |
| 패키지 | TOML 기반 스킬 패키지 (`package.toml` + `main.js`) |

### 샌드박스 프리미티브

스킬은 격리된 QuickJS VM에서 실행되며, 다음 프리미티브를 사용할 수 있습니다:

| 프리미티브 | 메서드 | 설명 |
|-----------|--------|------|
| `Http` | get, post, put, delete | HTTP 요청 (SSRF 보호) |
| `Web` | search, fetch | 웹 검색 + 콘텐츠 추출 |
| `Storage` | get, set, delete, list | 스킬별 키-값 저장소 |
| `Telegram` | sendMessage, sendPhoto, sendDocument | 텔레그램 메시지 |
| `Llm` | generate | LLM 호출 (실행당 3회 제한) |
| `File` | read, write | 패키지 데이터 디렉토리 파일 I/O |
| `Env` | get | 패키지 설정값 읽기 |

### 스킬 체이닝

스킬을 순차 파이프라인으로 연결할 수 있습니다:

```toml
# package.toml
[[chain]]
package = "fetch-data"

[[chain]]
package = "summarize"

[[chain]]
package = "send-telegram"
```

이전 단계의 출력이 다음 단계의 입력(`prev_output`)으로 전달됩니다.

---

## 데이터 저장 위치

```
~/.kittypaw/
├── kittypaw.db          # 대화 기록, 에이전트 상태 (SQLite)
├── secrets.json         # API 키, 채널 토큰 등 시크릿
├── packages/            # 설치된 스킬 패키지
│   ├── macro-economy-report/
│   │   ├── package.toml
│   │   ├── main.js
│   │   └── config.toml
│   ├── weather-briefing/
│   └── ...
└── skills/              # teach로 만든 스킬 (레거시)
```

API 키와 시크릿은 `~/.kittypaw/secrets.json`에 저장됩니다.

---

## 개발

```bash
cargo build              # 빌드
cargo test --lib          # 유닛 테스트 (142개)
cargo clippy              # 린트
cargo fmt                 # 포맷
```

### 프로젝트 구조

| Crate | 역할 |
|---|---|
| `kittypaw-core` | 타입, 설정, 패키지 매니저, 시크릿, 스킬 시스템 |
| `kittypaw-llm` | LLM 클라이언트 (Claude + OpenAI), 레지스트리 |
| `kittypaw-sandbox` | QuickJS VM + SkillResolver + OS 격리 |
| `kittypaw-store` | SQLite 저장소 (대화, 스킬 스토리지, 워크스페이스) |
| `kittypaw-engine` | 런타임 엔진 (agent loop, 스킬 실행기, 스케줄러, teach loop) |
| `kittypaw-workspace` | 파일 관리, 검색, 권한 체커 |
| `kittypaw-channels` | 텔레그램/슬랙/디스코드 채널 어댑터 |
| `kittypaw-cli` | CLI 바이너리, 서버 (serve.rs), 대시보드 API |
| `kittypaw-gui` | Dioxus 데스크톱 GUI |

---

## Acknowledgments

- [Whispree](https://github.com/Arsture/whispree) (MIT License) — Speech recognition architecture reference
- [agentskills.io](https://agentskills.io) — SKILL.md skill format standard

## 라이선스

MIT

---

*급할수록 필요한 건, 작고 빠른 고양이손* 🐱

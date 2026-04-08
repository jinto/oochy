# AI 에이전트 웹 검색: 유료 API 없이 되는가?

> 조사일: 2026-04-09
> 배경: KittyPaw TODO "온보딩에 검색 API 키 설정 추가" 방향 재검토

---

## TL;DR

**Hermes Agent와 OpenClaw 모두 DuckDuckGo를 무설정 폴백으로 내장하고 있다.**
유료 검색 API(Exa, Firecrawl, Brave)는 선택적 업그레이드일 뿐, 기본 동작에 필수가 아니다.
KittyPaw도 동일한 패턴을 따라야 한다 — "온보딩에 API 키를 물어보는" 대신 "DuckDuckGo로 그냥 작동하게" 만들어야 한다.

---

## 1. Hermes Agent의 검색 구현

| 백엔드 | API 키 | 비용 | 용도 |
|--------|--------|------|------|
| Firecrawl (기본) | `FIRECRAWL_API_KEY` | 유료 | search, extract, crawl |
| Exa | `EXA_API_KEY` | 유료 | search, extract |
| Parallel | `PARALLEL_API_KEY` | 유료 | search, extract |
| Tavily | `TAVILY_API_KEY` | 유료 | search, extract, crawl |
| **DuckDuckGo (폴백)** | **불필요** | **무료** | **search** |

**핵심 메커니즘**: DuckDuckGo 검색 스킬이 `fallback_for_toolsets: [web]`으로 내장.
유료 API 키가 하나도 없으면 DuckDuckGo가 자동 활성화된다.

Nous 구독자는 별도로 managed Firecrawl 게이트웨이를 제공받아 개별 API 키 없이 Firecrawl 품질의 검색을 사용할 수 있음.

**Sources**:
- [tools/web_tools.py](https://github.com/NousResearch/hermes-agent/blob/main/tools/web_tools.py)
- [Hermes 스킬 문서](https://hermes-agent.nousresearch.com/docs/user-guide/features/skills/)

---

## 2. OpenClaw / ZeroClaw의 검색 구현

**OpenClaw**: 12개 프로바이더 지원
- 유료 9종: Brave, Gemini, Perplexity, Tavily, Exa 등
- **무료 3종: DuckDuckGo, SearXNG, Ollama Web Search**
- 자동 감지 우선순위: Brave > Gemini > Perplexity > Grok
- v2026.3.22부터 DuckDuckGo & Exa가 번들 플러그인으로 기본 탑재

**ZeroClaw** (Rust 리라이트): 3개 프로바이더
- DuckDuckGo (무료, 키 불필요)
- Brave (유료)
- SearXNG (셀프호스트, 무료)

**Sources**:
- [OpenClaw 웹 검색 문서](https://docs.openclaw.ai/tools/web)
- [ZeroClaw web_search_tool.rs](https://github.com/zeroclaw-labs/zeroclaw/blob/main/src/tools/web_search_tool.rs)

---

## 3. 트위터/커뮤니티 간증 (20건+)

### "그냥 되더라" 계열

| 누가 | 뭐라 했나 | URL |
|------|-----------|-----|
| @fahdmirza | "Zero cloud, zero API keys -- 모든 것이 로컬 하드웨어에서" | [링크](https://x.com/fahdmirza/status/2035965754684895387) |
| @austin_hurwitz | "it just works out of the box" | [링크](https://x.com/austin_hurwitz/status/2033552632241857002) |
| @sudoingX | "hermes agent just works" — 소형 모델에서도 tool call 안정적 | [링크](https://x.com/sudoingX/status/2034518239443878076) |
| @jphorism | Enzyme 플러그인: "no account, no API key, no external service" | [링크](https://x.com/jphorism/status/2039822829412405671) |

### "SearXNG로 무료 무제한" 계열

| 누가 | 뭐라 했나 | URL |
|------|-----------|-----|
| @gregory_nico | "unlimited free web search. No API keys. No Brave. No monthly bills." SearXNG Docker 2분 셋업 | [링크](https://x.com/gregory_nico/status/2023377317226053709) |
| Th D ng (Medium) | "AI 프로바이더에 이미 많은 돈을 쓰고 있어 Brave까지 유료로 못함" → SearXNG | [링크](https://dzungvu.medium.com/openclaw-with-free-local-web-search-searxng-db52348b7d34) |
| @sixdayswest | OpenClaw에 SearXNG 추가 PR 직접 제출 | [링크](https://x.com/sixdayswest/status/2024604176483762271) |

### "완전 무료 실행" 계열

| 누가 | 뭐라 했나 | URL |
|------|-----------|-----|
| @JulianGoldieSEO | "completely FREE forever" — Atomic Bot + Ollama | [링크](https://x.com/JulianGoldieSEO/status/2038224653051650334) |
| @Zai_org (AutoClaw) | "No API key required" 명시적 광고 | [링크](https://x.com/Zai_org/status/2038632251551023250) |
| @socialwithaayan | "$0/month for local AI. No api bills, no rate limits" | [링크](https://x.com/socialwithaayan/status/2037438489852322012) |

### Brave 무료 폐지 후폭풍

| 누가 | 뭐라 했나 | URL |
|------|-----------|-----|
| @Kevin_Indig | "Brave 무료 API는 OpenClaw 봇들의 대량 사용으로 사라졌다" | [링크](https://x.com/Kevin_Indig/status/2022923386536575020) |
| @aakashgupta | "OpenClaw 운영에서 가장 비싼 게 Brave $5/월" | [링크](https://x.com/aakashgupta/status/2036493496291344431) |

### 커뮤니티 SearXNG 프로젝트 (GitHub 5개+)

| 프로젝트 | 설명 |
|----------|------|
| [openclaw-searxng](https://github.com/drawliin/openclaw-searxng) | "Free, privacy-respecting web search without Brave API key" |
| [openclaw-searxng-search](https://github.com/Shmayro/openclaw-searxng-search) | "Free, private, unlimited — no API keys, no rate limits" |
| [openclaw-free-web-search](https://github.com/wd041216-bit/openclaw-free-web-search) | "Zero-cost, zero-API-key, privacy-first" |
| [ask-search](https://github.com/ythx-101/ask-search) | "Self-hosted web search for AI agents — zero cost" |

---

## 4. 무료 검색 대안 비교 (2026년 4월 현재)

| 서비스 | 무료 한도 | API 키 | 신용카드 | 갱신 | 에이전트 적합도 |
|--------|-----------|--------|----------|------|----------------|
| **SearXNG** | **무제한** | 불필요 | 불필요 | - | 높음 (셀프호스팅) |
| **DuckDuckGo (ddgs)** | **무제한** (비공식) | 불필요 | 불필요 | - | 중간 (차단 위험) |
| Tavily | 월 1,000회 | 필요 | 불필요 | 매월 | 높음 |
| Jina AI | 10M 토큰 | 필요 | 불필요 | 일회성 | 중간 (20 RPM) |
| Serper.dev | 2,500회 | 필요 | 불필요 | 일회성 | 평가용 |
| Brave Search | ~1,000회 ($5) | 필요 | **필수** | 매월 | 유료화됨 |
| Exa AI | $10 크레딧 | 필요 | 미확인 | 일회성 | 시맨틱 특화 |
| Google CSE | 일 100회 | 필요 | 불필요 | 매일 | **신규 불가** |

**주요 변화**:
- Brave 무료 티어: 2026년 2월 **완전 폐지** (에이전트 봇 과다 사용이 원인)
- Google CSE: 신규 등록 불가, 2027년 1월 서비스 종료 예정
- Tavily: 2026년 2월 Nebius에 인수, 무료 티어 유지 중

---

## 5. 업계 패턴: 3-Tier 폴백 전략

Hermes와 OpenClaw가 수렴한 패턴:

```
Tier 1: DuckDuckGo (무설정, 즉시 작동)
  ↓ 품질 부족하면
Tier 2: SearXNG (파워유저, 셀프호스팅)
  ↓ 더 좋은 품질 원하면
Tier 3: Brave / Tavily / Exa (유료 API 키)
```

**사용자 경험 흐름**:
1. 첫 설치: 아무것도 안 물어봄 → DuckDuckGo로 검색이 "그냥 됨"
2. 파워유저: SearXNG Docker 띄워서 무제한 무료 검색
3. 프로: Brave/Tavily 키 입력해서 프리미엄 검색

---

## 6. KittyPaw에 대한 시사점

### 현재 TODO 항목 (재검토 필요)

> "온보딩에 검색 API 키 설정 추가 — Web.search 백엔드(Brave/Tavily/Exa) API 키를 온보딩 위자드에서 입력받도록"

### 문제

이 방향은 KittyPaw 철학("보이지 않는 AI", "5분 안에 설치하고 1주 후 AI가 있다는 걸 잊었는가")과 **정면 모순**이다.

1. 온보딩에 검색 API 키 단계를 추가하면 → 설치 마찰 증가
2. "API 키가 뭔데?"라고 물을 타겟 사용자(기술 인접 파워유저)에게 장벽
3. Hermes/OpenClaw 모두 이미 "키 없이 작동" 방향으로 수렴

### 제안: TODO 수정

기존:
```
온보딩에 검색 API 키 설정 추가
```

변경:
```
Web.search 무설정 기본값 (DuckDuckGo) + 고급 설정에서 유료 API 선택적 지원
```

구체적으로:
1. **DuckDuckGo를 Web.search 기본 백엔드로** — API 키 불필요, 온보딩 변경 없음
2. **Settings 고급 탭에 검색 백엔드 선택** — Brave/Tavily/SearXNG URL 입력 (선택적)
3. **SearXNG 지원 추가** — 파워유저를 위한 무제한 무료 옵션
4. **폴백 체인 구현** — 설정된 유료 API 실패 시 → DuckDuckGo 자동 폴백

이 방향이 KittyPaw의 "Silent Engine" 철학, 경쟁사 패턴, 사용자 기대 모두에 부합한다.

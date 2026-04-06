# 경쟁사 "와우" 사용 사례 vs KittyPaw 달성 가능성

## Hermes Agent

| # | 사용 사례 | 와우 포인트 | KittyPaw 현황 | 가능? |
|---|----------|-----------|-------------|------|
| 1 | **개인 AI 뉴스 브리핑** | HN 기사 → 관심사 랭킹 → 오디오 요약 → 매일 아침 Telegram | trend-scanner + content-drafter 체인 + Tts.speak + Telegram.sendVoice | ✅ 텍스트+음성 브리핑 모두 가능 (edge-tts-rust, 무료) |
| 2 | **트렌딩 오픈소스 리포트** | Reddit + X → AI 트렌드 Top 5 → 1시간 셋업 | Web.search + Http.get + Llm.generate + Telegram.sendMessage | ✅ 스킬로 즉시 구현 가능 |
| 3 | **자가 학습 스킬** | 복잡한 작업 완료 후 스스로 스킬 생성 → 다음에 자동 실행 | 자가 개선 루프 (2회 실패 시 LLM 수정) 있음 | ⚠️ "수정"은 되지만 "새 스킬 자동 생성"은 없음 |
| 4 | **멀티채널 도달** | Telegram/Discord/Slack/WhatsApp/Signal + $5 VPS | Telegram/Slack/Discord 3채널 + daemon | ✅ 3채널. WhatsApp/Signal은 미지원 |
| 5 | **$5 VPS 24시간 운영** | 저렴한 서버에서 상시 운영 | kittypaw daemon + kittypaw serve | ✅ 가능 |
| 6 | **기억 시스템** | MEMORY.md + USER.md + 전체 세션 SQLite 검색 | user_context + execution_history + FTS5 검색 | ✅ 동등 |

## OpenClaw / ZeroClaw

| # | 사용 사례 | 와우 포인트 | KittyPaw 현황 | 가능? |
|---|----------|-----------|-------------|------|
| 7 | **멀티에이전트 팀** | 계획 에이전트 → 전문 에이전트 병렬 실행 → 결과 합산 | Agent.delegate (순차, 깊이 2) + chain | ⚠️ 순차만. 병렬 위임 없음 |
| 8 | **ClawFlows 111개** | 프리빌트 워크플로우 설치만 하면 바로 사용 | 레지스트리 15개 스킬 | ⚠️ 수 부족 (15 vs 111) |
| 9 | **13k+ 스킬 생태계** | 커뮤니티 스킬 13,000개+ | SKILL.md 호환으로 접근 가능하나 자체 생태계 작음 | ⚠️ 포맷 호환은 됨, 자체 수 부족 |
| 10 | **프로덕션 자동화** | 코딩, 리서치, 파일, 이메일, API 전부 자동화 | Shell + Git + File + Http + Web + Llm | ✅ 프리미티브 충분 |

## Claude Code (참고 — 코딩 에이전트, 다른 카테고리)

| # | 사용 사례 | 와우 포인트 | KittyPaw 현황 | 가능? |
|---|----------|-----------|-------------|------|
| 11 | **자율 코드 리뷰** | 병렬 버그 탐지 + 심각도 랭킹 | KittyPaw 범위 아님 (코딩 에이전트 아님) | N/A |
| 12 | **/loop 야간 작업** | 자동 배포, 테스트, 보안 감사 | schedule loop + daemon | ✅ 동일 기능 |

---

## 달성 요약

| 상태 | 수 | 사용 사례 |
|------|---|----------|
| ✅ 지금 가능 | 10 | 텍스트+음성 브리핑, 트렌딩 리포트, 멀티채널, VPS, 기억, 야간 작업, 이미지, 비전, MoA, 프로필 |
| ⚠️ 부분 가능 | 3 | 스킬 자동 생성, 병렬 위임, 생태계 수 |
| ❌ 불가 | 0 | - |
| N/A | 2 | 코딩 에이전트 전용 |

## 남은 갭 (구현 우선순위)

1. ~~**TTS (Text-to-Speech)**~~ ✅ 완료 — `Tts.speak` + `Telegram.sendVoice` (edge-tts-rust, 무료)
2. **스킬 자동 생성 (Procedural Memory)** — 복잡한 작업 완료 후 teach_loop 자동 호출
3. **Agent.delegate 병렬** — 여러 서브에이전트를 동시에 실행
4. **스킬 수 확대** — 커뮤니티 + 큐레이션 스킬 100개 목표

## 출처

- [awesome-hermes-agent](https://github.com/0xNyk/awesome-hermes-agent)
- [Hermes Agent Complete Guide](https://virtualuncle.com/hermes-agent-complete-guide-2026/)
- [OpenClaw in Production](https://dev.to/virtualunc/openclaw-in-production-real-costs-security-setup-and-what-a-month-of-daily-use-actually-looks-10dg)
- [ClawFlows: 111 Prebuilt Workflows](https://www.sitepoint.com/clawflows-prebuilt-ai-workflows-openclaw/)
- [OpenClaw vs Hermes Agent](https://turingpost.substack.com/p/ai-101-hermes-agent-openclaws-rival)

# Design System — KittyPaw

## Product Context
- **What this is:** 비개발자/기술 인접 파워유저를 위한 데스크톱 AI 자동화 앱
- **Who it's for:** 권한 부여나 트러블슈팅은 하지만 터미널/config/Docker는 싫어하는 사람
- **Space/industry:** AI agent automation (vs. Hermes Agent, OpenClaw, Atomic Bot, Thoth)
- **Project type:** Native desktop app (Dioxus/Rust, macOS)

## Aesthetic Direction
- **Direction:** Industrial/Utilitarian — 공장의 컨트롤 패널처럼. 화려함이 아니라 신뢰감.
- **Decoration level:** Minimal — 타이포그래피와 여백이 모든 구조를 만든다.
- **Mood:** Cozy tech. 따뜻하지만 정밀한. 결과가 주인공이고 AI는 조용히 돌아간다.
- **Reference sites:** Raycast(절제+프리미엄), Thoth(knowledge graph), Atomic Bot(원클릭)

## Typography
- **Display/Hero:** Fraunces (variable, optical size 9-144) — 온기 있는 세리프. "cozy tech" 브랜드. 기술 도구에 세리프는 이례적이지만 차갑고 기계적인 느낌을 깨고 KittyPaw만의 인격을 준다.
- **Body:** Inter — 가독성과 UI 밀도에 검증된 선택. system font fallback.
- **UI/Labels:** Inter (same as body)
- **Data/Tables:** Geist Mono — Vercel이 만든 모노스페이스. tabular-nums 지원. 시간, 숫자, 스케줄 표시에 최적.
- **Code:** Geist Mono
- **Loading:** Google Fonts CDN (Fraunces, Geist Mono). Inter는 system font fallback 우선.
- **Scale:**
  - xs: 11px / 0.6875rem
  - sm: 12px / 0.75rem (labels, timestamps)
  - base: 13px / 0.8125rem (body, UI)
  - md: 15px / 0.9375rem (section titles)
  - lg: 17px / 1.0625rem (sidebar logo)
  - xl: 24px / 1.5rem (page titles — Fraunces)
  - 2xl: 28px / 1.75rem (hero numbers — Geist Mono)

## Color
- **Approach:** Restrained — 색은 의미 있을 때만. 기본은 워뮨 그레이.
- **Background:** #F5F3F0 (warm stone-50)
- **Surface:** #FFFFFF (cards, panels)
- **Sidebar:** #1C1917 (stone-900)
- **Sidebar hover:** #292524 (stone-800)
- **Text:** #1C1917 (stone-900)
- **Text muted:** #78716C (stone-500)
- **Text sidebar:** #D6D3D1 (stone-300)
- **Border:** #E7E5E4 (stone-200)
- **Accent (primary):** #86EFAC (green-300) — "돌아가고 있다"는 생명력. 성공, 활성 상태.
- **Accent background:** #F0FDF4 (green-50) — 조용한 개선 배너, 강조 카드.
- **Accent text:** #166534 (green-800) — 액센트 배경 위의 텍스트.
- **Semantic:**
  - Success: #86EFAC (green-300)
  - Warning: #FDE68A (amber-200) / bg: #FFFBEB
  - Error: #FCA5A5 (red-300) / bg: #FEF2F2
  - Info: #93C5FD (blue-300) / bg: #EFF6FF
- **Dark mode strategy:** v2에서 도입. 사이드바 색상을 배경으로 확장, 서피스를 stone-800으로, 텍스트를 반전. 액센트 채도 10-20% 감소.

## Spacing
- **Base unit:** 8px
- **Density:** Comfortable — 대시보드에 정보가 많지만 숨을 공간이 있다.
- **Scale:**
  - 2xs: 2px
  - xs: 4px
  - sm: 8px
  - md: 14px
  - lg: 20px
  - xl: 28px
  - 2xl: 32px
  - 3xl: 48px

## Layout
- **Approach:** Grid-disciplined — 대시보드이므로 예측 가능한 정렬.
- **Structure:** 고정 사이드바(220px) + 스크롤 메인 영역
- **Grid:** 메인 영역 내 3-column grid (stat cards), 1-column list (skills, activity)
- **Max content width:** 제한 없음 (사이드바 제외한 전체 너비 사용)
- **Border radius:**
  - sm: 6px (buttons, inputs, tags)
  - md: 10px (cards, panels)
  - lg: 14px (modal, dialog)
  - full: 9999px (status dots, pills)

## Motion
- **Approach:** Minimal-functional — 상태 변화만. 화면 전환 시 부드러운 페이드.
- **Easing:** enter(ease-out) exit(ease-in) move(ease-in-out)
- **Duration:**
  - micro: 80ms (hover state, button press)
  - short: 150ms (tab switch, panel open)
  - medium: 250ms (page transition)
- **Status dot animation:** running 상태일 때 subtle pulse (box-shadow glow)

## Dashboard Layout (Initial Screen)
- **Header:** "Good morning" (Fraunces) + 날짜 (오른쪽 정렬)
- **Stat cards (3-column):** Active Skills / Today's Runs / Silent Optimizations(accent)
- **Skills list:** status dot + name + last result + schedule + tag
- **Quiet banner:** "이번 주 N번의 조용한 개선 적용됨" + 자세히 보기
- **Recent activity:** 시간순 로그, 조용한 개선 항목은 초록색 강조

## Navigation (Sidebar)
1. Dashboard (기본 화면)
2. Skills (스킬 스토어 + 설치된 스킬 관리)
3. Chat (스킬 설정/커스터마이즈/디버깅 전용)
4. Settings (API 키, 모델, 일반 설정)

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-03-30 | Dashboard-first home screen | 철학: "주인공은 결과." 채팅이 아닌 자동화 현황판이 첫 화면. |
| 2026-03-30 | Light mode default | 경쟁자 전부 다크 모드. 라이트로 차별화. Cozy tech 무드에 적합. |
| 2026-03-30 | Fraunces serif for display | 기술 도구에 세리프는 이례적이지만 "cozy tech" 브랜드에 온기. |
| 2026-03-30 | Soft green (#86EFAC) accent | 네온 라임(Atomic Bot)이나 골드(Thoth)가 아닌 자연스러운 초록. "살아있다". |
| 2026-03-30 | Chat as 3rd tab | AI는 조용해야 한다. 채팅은 설정/디버깅 도구. |

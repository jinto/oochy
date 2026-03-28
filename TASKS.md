# KittyPaw Tasks

## In Progress

### Skill Platform — 설치 가능한 자동화 패키지 시스템
- **목표**: 비개발자가 GUI에서 스킬 갤러리 → 설치 → 설정 → 실행할 수 있는 플랫폼
- **영감**: [Macro-Pulse](https://github.com/yeseoLee/Macro-Pulse) 같은 자동화를 코드 없이
- **플랜**: `.omc/plans/skill-platform.md`
- **컨센서스**: Architect + Critic 리뷰 완료 (REVISE → 수정 반영)

#### Phase 0: 샌드박스 데이터 플로우 수정 (BLOCKER)
- [ ] `run_child_async`에 `SkillResolver` 콜백 추가
- [ ] skill stub이 실제 Http/Storage/Llm 응답을 JS에 반환
- [ ] resolver 없으면 기존 fire-and-forget 유지 (하위 호환)
- [ ] `agent_loop.rs`에서 resolver 구성 (Config, Store, Http 접근)

#### Phase 1: 패키지 포맷 + 백엔드 매니저
- [ ] `SkillPackage`, `PackageMeta`, `ConfigField` 타입 (kittypaw-core)
- [ ] `package_manager.rs` — install/uninstall/configure/list
- [ ] 패키지 → 기존 skill executor 브릿지 (ctx.config.* 주입)
- [ ] `load_all_packages()` + 스케줄러 통합
- [ ] OS keychain으로 시크릿 저장 (`keyring` crate)
- [ ] CapabilityChecker ← 패키지 permissions 매핑

#### Phase 2: 샌드박스 확장
- [ ] `File.write(path, content)` / `File.read(path)` — 패키지 data 디렉토리 스코핑
- [ ] `Telegram.sendDocument(chatId, fileUrl, caption)`
- [ ] `Env.get(key)` — 패키지 config 읽기
- [ ] host-side handler 구현 (skill_executor.rs)

#### Phase 3: GUI — 스킬 갤러리 + 설정 위자드
- [ ] Tauri commands (list/install/uninstall/configure/test-run/toggle)
- [ ] `SkillGallery.svelte` — 카테고리 탭, 설치 버튼
- [ ] `SkillConfig.svelte` — 자동 생성 폼 (config schema 기반)
- [ ] 테스트 실행 + 결과 패널

#### Phase 4: 예제 패키지 5개
- [ ] macro-economy-report (FRED API + Telegram)
- [ ] weather-briefing (OpenWeatherMap + Telegram)
- [ ] rss-digest (RSS + LLM 요약 + Telegram)
- [ ] reminder (키워드 트리거 + Storage)
- [ ] url-monitor (상태 체크 + 알림)

#### Phase 5: 폴리시 + 배포
- [ ] 에러 처리, 온보딩, 문서
- [ ] GitHub 기반 배포 (registry index + `kittypaw install github:user/repo`)

## Backlog

### Multi-model routing (pi-gigaplan 스타일)
- **참고**: https://github.com/umgbhalla/pi-gigaplan, https://github.com/badlogic/pi
- 프롬프트 유형(reasoning, coding, design 등)에 따라 최적 모델로 자동 라우팅
- `kittypaw-llm` 크레이트에 OpenAI API 클라이언트 추가
- 프롬프트 분류기 + 모델 라우팅 테이블 (설정 파일 기반)
- 수동 모델 전환 커맨드 지원

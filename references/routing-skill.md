# model·effort 라우팅 (soksak)

일의 난이도를 판정해 각 실행 단위의 **model 과 reasoning effort 를 명시로 선택**한다. 부모 프로필을
무작정 상속하지 마라. provider 자동선택(intelligence/speed/**price** 균형)에 위임하지 마라 — price 가
개입해 품질을 깎는다. 상속도 자동도 함정이다. **명시 선택만이 최고 요구 작업을 최고로 굴린다.**

## 난이도 4축

각 작업을 네 축으로 읽는다. 평균 내지 마라 — **가장 강한 신호 하나로 판정한다**(한 축이 심각하면 그 축이 tier).

- **명확성**: 원하는 동작과 구현 경로가 이미 정해졌는가.
- **범위**: 국소·기계적인가, 교차모듈·아키텍처인가.
- **위험**: 폭발 반경·되돌림 가능성·보안·프로덕션 노출·규제/법정 의무·불가역성.
- **검증 난이도**: 결과 검증과 상충 증거 해소가 얼마나 어려운가.

## provider 별 선택표 (실측, 2026-07-12)

두 provider 의 effort 어휘가 다르다. 최고 tier 도 다르다(claude `max`, codex `ultra`).

| 난이도 | codex model | codex effort | claude model | claude effort |
|---|---|---|---|---|
| 기계적·저위험·되돌리기 쉬움 | `gpt-5.6-luna` | low (±minimal) | haiku/sonnet | low/medium |
| 일반 구현·탐색·교차모듈 | `gpt-5.6-terra` | medium/high | sonnet | medium/high |
| 아키텍처·보안·프로덕션위험·규제/법정 의무·개방형 판단 | `gpt-5.6-sol` | high~ultra | opus | high/max |
| 한턴에 고민하는 저작(JSON·플랜) | `gpt-5.6-sol` | **ultra** | opus | **max** |

- codex effort 값 = `low·medium·high·xhigh·max·ultra`(+minimal/none). `-c model_reasoning_effort=<v>`.
- claude effort 값 = `low·medium·high·xhigh·max`. `--effort <v>`.
- **미지정 = 최고**(품질우선). 낮추는 것은 순수 기계적 작업에 대해서만, 명시적으로. 되돌리기 비싼 실수가
  더 큰 재작업을 부르는 곳에 추론을 굶기지 마라.
- **규제 준수·법정 기록 보존·불가역 데이터 무결성 = max(claude)/ultra(codex)** — 되돌릴 수 없고 법적 책임이
  걸린다. 이 부류를 high 로 눌러 앉히지 마라.

## 병렬 규율

- **독립·저위험·기계검증 가능** 작업만 병렬. **중요 판정은 단일 책임자.**
- 부모는 위임 결과를 **전수 검증한 뒤** 채택한다 — 무검증 채택 금지.
- **동시 쓰기 작업은 격리 체크아웃(worktree)**. 같은 체크아웃 동시 쓰기 금지. (codex: `[agents] max_depth=1`
  로 자손 스폰 차단, `sandbox` 로 권한 클램프. claude: `isolation:worktree`.)
- **유계 fanout**: 기본 0, 보통 1, 고위험 다단계 2, 승인된 사고·명시 요청만 3. (codex `[agents] max_threads`.)

## 승격 vs 재시도

- 같은 model·effort 재시도는 transient 실패·불완전 브리프에만. 전체 재실행보다 국소 정정을 우선.
- 경로는 유효한데 추론 깊이가 모자랐으면 effort 를 올린다.
- 판단이 무거워지면 Luna→Terra(claude sonnet→opus). 증거가 상충하거나 아키텍처가 바뀌면 Terra→Sol.
- **어떤 경우에도 model·effort 를 무음으로 낮추지 마라.** quota·정책·가용성·지연·비용이 대체를 강제하면 보고한다.

## soksak 워크플로 적용

저작 LLM 은 DAG 를 만들 때 각 노드/task 의 난이도를 안다. 그 판정을 노드에 싣는다 — 노드의 `effort`
(그리고 필요 시 `model`) tier. reconcile 이 그 tier 를 실행자에게 흘려보내고, 실행자가 provider 별로
honor 한다(claude `--effort`, codex `-c model_reasoning_effort`). tier 미지정 노드는 기본 최고로 실행된다.

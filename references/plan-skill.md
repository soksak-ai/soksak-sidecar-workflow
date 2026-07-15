# plan(한턴 슈도코드화) — 설계 설명서

**정본은 `workflows/research.doc.json` 의 `plan` stage 다.** 이 문서는 설명이다 — 어긋나면 doc 이 옳다
(프롬프트 원문을 여기 복사하지 않는다).

## 역할

plan 은 **워크플로가 아니라 한 턴이다** — research 가 확정한 기초지식({{facts}})과 드래프트 요건
원장({{ledger}})을 받아 **한 번의 agent 호출로 슈도코드화**한다. 산출 = 실제 개발 업무의 단위가 될
plan-unit 노드들. 이슈라이즈(`workflow.issuerize`)가 이것을 unlock 개발 이슈로 승격한다.

## 계약 (doc 이 강제)

- plan task 는 research stage 가 발행한다(stage='plan', blockedBy=[factIds]) — fact 전부 badge 확정
  (o/x/f)이어야 ready(두 축 분리 §4: badge 보유 노드의 done=badge).
- reconcile 이 plan stage 실행 시 **요건 원장(args.ledger)과 fact 원장(args.facts, kind=fact)을 필수
  주입**한다 — 원장 없이 슈도코드화하지 않는다(fail-loud §2).
- 산출 kind = `plan-unit`: badge 없음(검증 파이프 비대상), locked(덩어리 응집), title=업무 단위명,
  description=**슈도코드 전문** — 대상 요건([iN])·근거 fact([factN])·acceptance 를 본문에 인용해
  self-contained(빌더가 이 unit 하나만 보고 착수). "testing/deploy" 같은 phase-unit 금지 — 검증은
  각 unit 의 acceptance 줄로.
- 재진입 멱등: reconcile 마커 = 덩어리 직속 plan-unit 존재.

## 이슈라이즈 연결

`workflow.issuerize {chunk}` 게이트(PRINCIPLES §5): badge='o' ∧ fact 전부 검증 ∧ plan-unit ≥1 ∧
미승격(멱등). 승격 = plan-unit 별 unlock 이슈(kind=issue, parentDraftId 계보, 슈도코드+o 확정
배경지식 동반). 실제 개발 실행은 이슈 소비자 몫.

## 수정 시 규칙

research-skill.md 의 「수정 시 규칙」과 동일 — 프롬프트 byte 안정성(§3)·계약 변경 시 테스트 동반(§6).

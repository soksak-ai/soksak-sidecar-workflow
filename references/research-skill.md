# research 워크플로 — 설계 설명서

**정본은 `workflows/research.doc.json` 이다.** 이 문서는 정본이 아니라 그 계약의 설명이다 — 여기 서술과
doc 이 어긋나면 doc 이 옳고, 이 문서를 고친다(중복 서술 금지 — 프롬프트 원문을 여기 복사하지 않는다).

## 역할

research 는 **인증된 드래프트 덩어리(audit badge='o')** 를 입력으로, 실제 개발이 딛고 설 **기초지식을
확정하는 상태**로 워크플로를 태운다: framework(스택/저장소 선정), methodology(검증·게이트·배포 규율),
directive(도메인 법/규정/표준 의무) — 각 fact 는 근거·출처를 달고 draft 항목과 **같은 검증 파이프**
(badge 검수전 → exec-one → o/x/f)를 통과해야 "확정"이다.

research 끝에 **design 체인**이 이어진다(대회 실측 채택 방법론 — 통합 한턴은 재현 결정성 실패,
병렬은 이음새 상호 모순으로 기각): `design-interface → design-domain → design-criteria` 순서로
각 스텝이 design fact(kind=fact, category=interface/domain-model/criterion)를 발행하고 다음 task 를
건다(blockedBy=자기 factIds). 뒤 스텝은 앞 산출을 {{facts}} 원장으로 계승한다 — 글자 단위 이음새
정합이 이 체인의 존재 이유다. 체인 말미(criteria)가 plan task 를 발행해, plan(한턴 파일별 슈도코드화,
plan-skill.md)이 이어진다.

## 계약 (doc 이 강제)

- 산출 kind = `fact` — draft 요건(kind=item)과 원장이 섞이지 않는다(buildLedger 는 kind 필터).
  fact 의 area(framework/methodology/directive)는 category 필드로 발행된다.
- 저작 LLM 불참(PRINCIPLES §7): 입력 directive 가 이미 정련본이라 재정련할 것이 없다 — canonical doc
  정적 인스턴스화만 한다. 실행 통로는 `workflow.research {chunk}` (서비스 serve 핸들러):
  게이트(researchGate — badge='o' ∧ description 존재 ∧ 멱등) → `--workflow research --emit
  --args-json {chunkRef, directive}` → 발행 relay. task body는 doc 임베드가 아니라
  `{workflow:"research"}` 이름 참조이며, exec-stage는 바이너리에 포함된 선언 문서를 로드한다.
- directive = 덩어리 description(정련 정본) — 단일 진실(PRINCIPLES §1).
- research stage 는 reconcile 이 인증 요건 원장(args.ledger)을 필수 주입한다(발굴 근거).
  프롬프트 정규화는 draft 와 동형: FACT_VERIFY_TMPL 1행(registerPromptsOnce) + directive(varRefs) +
  항목 vars(title/description).
- 재진입 멱등: reconcile 마커 = 덩어리 직속 fact 존재(research 가 fact 의 유일한 출처).

## 수정 시 규칙

- 프롬프트 원문(RESEARCH_COMMON·FACT_VERIFY_TMPL·prompts.research)의 byte 안정성은 콘텐츠 주소화
  dedup 의 전제다(PRINCIPLES §3) — 다듬기 변경은 전 research 의 dedup 을 깨는 변경이며 의도적일 때만.
- 스키마(RESEARCH_SCHEMA·VERIFY_SCHEMA)와 stages 구조 변경은 소비자(서비스 reconcile 분기·검증
  파이프·이슈라이즈 게이트)와의 계약 변경이다 — 해당 테스트(doc_interp `bundled_research_doc_*`,
  reconcile.rs research/plan 케이스)를 함께 고친다(기준 약화 금지 §6).

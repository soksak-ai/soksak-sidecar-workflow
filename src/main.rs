//! soksak-sidecar-workflow — 워크플로 사이드카 CLI. **workflow-doc@0.0.1**(언어중립 JSON 문서, doc_interp) 단일 경로 — stage 별로
//! 실행해 (a) 노드 DAG 를 *발행*(--emit)하거나 (b) stage/노드를 *실행*(exec-stage/exec-one)한다. agent 는
//! claude -p. 실행 오케스트레이션은 코어 스케줄러가 맡고 이 런타임은 선언 문서만 실행한다.
//!
//!   soksak-sidecar-workflow <doc.json|-> --emit [--args-json {...}] [--lang ko]     # 노드 DAG 발행(stdout JSON line, LLM 미호출)
//!   soksak-sidecar-workflow --workflow <name> --emit [--args-json {...}]            # 번들 정본(workflows/<name>.doc.json) 발행
//!   soksak-sidecar-workflow exec-one  [--lang ko] [--model m] [--allow-tools "..."] # stdin {prompt, schema?} 한 노드 실행 → {oxf, result} (스케줄러가 ready 노드에)
//!   soksak-sidecar-workflow exec-stage [--lang ko] [--model m]                      # stdin {skeleton, stage, args} stage 실행 → 자식 {ev:add} + {ev:result}
//!   soksak-sidecar-workflow synth --idea "..."                                      # ③파생 도메인 지시어만
//! 인증 env(ANTHROPIC_*)는 호출자가 export.

use serde_json::{json, Map, Value};
use soksak_sidecar_workflow::author_doc::build_user_prompt;
use soksak_sidecar_workflow::derive_directive::derive_directives;
use soksak_sidecar_workflow::domain_lib::builtin_library;
use soksak_sidecar_workflow::exec_one;
use soksak_sidecar_workflow::lang::Language;
use soksak_sidecar_workflow::paths::bundled_workflow;
use soksak_sidecar_workflow::prompt_assembly::build_prompt_with_schema;
use soksak_sidecar_workflow::provider::{run_agent, run_agent_text, AgentRequest};
use std::collections::HashSet;

const DEFAULT_MODEL: &str = "opus"; // 실제 모델은 인증 프로필이 매핑

fn main() {
    if let Err(e) = real_main() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// auth_env — claude -p 호출용 인증 env 수집 + 토큰 확인. ANTHROPIC_AUTH_TOKEN 프로필을 OAuth 프로필보다 우선.
/// 환경에 두 토큰이 공존할 때(래퍼가 ANTHROPIC_* 를 주입해도 zshrc 의 OAUTH 가 잔류) 토큰 프로필이 쓰이도록 —
/// ANTHROPIC_AUTH_TOKEN 있으면 그 프로필 확정 + OAUTH 를 env 에서 제외(혼합 → claude 오판 회피). 없으면 OAuth.
fn auth_env() -> Result<(Vec<(String, String)>, &'static str), String> {
    let all: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("ANTHROPIC_")
                || k == "CLAUDE_ACCOUNT_NAME"
                || k == "CLAUDE_CODE_OAUTH_TOKEN"
        })
        .collect();
    let has_token = all.iter().any(|(k, _)| k == "ANTHROPIC_AUTH_TOKEN");
    let has_oauth = all.iter().any(|(k, _)| k == "CLAUDE_CODE_OAUTH_TOKEN");
    // codex provider 는 자체 로그인(~/.codex)을 쓴다 — ANTHROPIC 계열 토큰을 요구하지 않는다.
    let is_codex = std::env::var("SOKSAK_WORKFLOW_PROVIDER").ok().as_deref() == Some("codex");
    if !has_token && !has_oauth && !is_codex {
        return Err("프로필 인증 토큰 미설정 — ANTHROPIC_AUTH_TOKEN 또는 CLAUDE_CODE_OAUTH_TOKEN export 후 실행하라".to_string());
    }
    // 토큰 프로필 우선: ANTHROPIC_AUTH_TOKEN 있으면 그 env 만. OAUTH 가 잔류해도 제외(혼합 방지).
    let (env, profile) = if has_token {
        (
            all.into_iter()
                .filter(|(k, _)| k.starts_with("ANTHROPIC_") || k == "CLAUDE_ACCOUNT_NAME")
                .collect(),
            "token",
        )
    } else {
        (all, "oauth")
    };
    Ok((env, profile))
}

fn real_main() -> Result<(), String> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--handshake") {
        println!(
            "{}",
            serde_json::to_string(&soksak_sidecar_workflow::interface::handshake())
                .map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        eprintln!("usage:");
        eprintln!("  soksak-sidecar-workflow <doc.json|-> --emit [--args-json {{...}}] [--lang ko]                                  # workflow-doc 발행(stdout JSON line, LLM 미호출)");
        eprintln!("  soksak-sidecar-workflow --workflow <name> --emit [--args-json {{...}}] [--lang ko]                             # 번들 정본(workflows/<name>.doc.json) 발행");
        eprintln!("  soksak-sidecar-workflow generate-skeleton --idea \"...\" [--model m] [--lang ko] [--gen-out p] [--refs dir]    # 아이디어 → workflow-doc(LLM 저작·검증) JSON stdout");
        eprintln!("  soksak-sidecar-workflow exec-one [--lang ko] [--model m] [--allow-tools \"...\"]                                # stdin {{prompt,schema?}} 한 노드 실행 → {{oxf,result}}");
        eprintln!("  soksak-sidecar-workflow exec-stage [--lang ko] [--model m]                                                    # stdin {{skeleton,stage,args}} stage 실행 → 자식 {{ev:add}} + {{ev:result}}");
        eprintln!("  soksak-sidecar-workflow draft-run --idea \"...\" [--lang ko] [--model m] [--dump p]                                        # 앱/보드 없이 DRAFT 흐름 완주(인메모리 보드+실 provider; 인증 env 필수)");
        eprintln!("  soksak-sidecar-workflow synth --idea \"...\"                                                                    # ③파생 도메인 지시어");
        eprintln!("  --lang: 출력 언어 계약. 모든 agent 프롬프트에 주입 → 산출물이 그 언어로. args.lang 도 주입.");
        return Ok(());
    }
    // synth — ③파생만(LLM 미호출).
    if argv[0] == "synth" {
        let mut idea = String::new();
        let mut i = 1;
        while i < argv.len() {
            if argv[i] == "--idea" {
                i += 1;
                idea = argv.get(i).cloned().ok_or("--idea 값 누락")?;
            }
            i += 1;
        }
        if idea.is_empty() {
            return Err("synth: --idea 필수".to_string());
        }
        let directives = derive_directives(&idea, &builtin_library());
        println!(
            "{}",
            serde_json::to_string_pretty(&directives).map_err(|e| e.to_string())?
        );
        return Ok(());
    }

    // serve — 코어가 스폰하는 상주 서비스. stdio NDJSON로 커맨드와 상태를 소유한다.
    if argv[0] == "serve" {
        return soksak_sidecar_workflow::wf_service::run_serve();
    }
    // exec-one — 단일 노드 실행(규칙 C). stdin {prompt, schema?, model?} → claude → {oxf, result}.
    // 발행과 분리된 stateless 실행기. 코어 스케줄러가 칸반 ready 노드 하나를 이 경로로 실행한다.
    if argv[0] == "exec-one" {
        return run_exec_one(&argv);
    }

    // exec-stage — stage 작업 실행(동적 발행). stdin {workflow|skeleton(doc), stage, args:{directive, chunkRef, ledger…}}
    // → doc_interp 로 stage 실행(agent=claude, publish=NodeEvent) → 자식 {ev:add} JSON line + 최종 {ev:result}
    // 서비스 reconcile 이 kind=task 노드를 이 경로로 실행해 항목/fact 를 동적 발행한다.
    if argv[0] == "exec-stage" {
        return run_exec_stage(&argv);
    }

    // generate-skeleton — 아이디어 → workflow-doc@0.0.1(LLM 저작) → validate(fail-loud) → doc JSON stdout.
    // system=workflow-doc 저작 스킬 + soksak draft-skill.md(역할), user=아이디어+③파생.
    if argv[0] == "generate-skeleton" {
        return run_generate_skeleton(&argv);
    }
    // draft-run — 앱/보드 없이 DRAFT 흐름을 in-process 로 끝까지(변경 0 수렴) 돌린다.
    // idea → generate_skeleton(실 LLM) → 인메모리 보드 발행 → reconcile_tick 반복. 인증 env 필수.
    if argv[0] == "draft-run" {
        return run_draft_run(&argv);
    }
    // build-ledger — reconcile 순수 로직을 노출하는 LLM 없는 CLI 경계.
    if argv[0] == "build-ledger" {
        return run_build_ledger(&argv);
    }

    // --workflow <name> — 바이너리에 포함된 정본 워크플로를 이름으로 해석한다.
    // research/plan 처럼 저작 LLM 불참(PRINCIPLES §7) canonical doc 의 실행 통로.
    let (bundled, path, arg_start) = if argv[0] == "--workflow" {
        let name = argv.get(1).ok_or("--workflow 값(이름) 누락")?;
        (
            Some(soksak_sidecar_workflow::paths::bundled_workflow(name)?),
            String::new(),
            2usize,
        )
    } else {
        (None, argv[0].clone(), 1usize)
    };
    let mut args: Map<String, Value> = Map::new();
    let mut args_override: Option<Value> = None; // --args-json: cc 계약대로 args 를 verbatim(임의 JSON)
    let mut lang: Option<Language> = None; // --lang: 출력 언어 계약
    let mut emit = false; // --emit: 노드 발행만 + stdout JSON line. LLM 미호출.
    let mut i = arg_start;
    while i < argv.len() {
        match argv[i].as_str() {
            "--arg" => {
                i += 1;
                let kv = argv.get(i).ok_or("--arg 값 누락")?;
                let (k, v) = kv.split_once('=').ok_or("--arg 는 KEY=VALUE 형식")?;
                args.insert(k.to_string(), Value::String(v.to_string()));
            }
            "--args-json" => {
                i += 1;
                let j = argv.get(i).ok_or("--args-json 값 누락")?;
                args_override =
                    Some(serde_json::from_str(j).map_err(|e| format!("--args-json 파싱: {e}"))?);
            }
            "--lang" => {
                i += 1;
                let v = argv.get(i).ok_or("--lang 값 누락")?;
                lang = Some(Language::parse(v));
            }
            "--emit" => emit = true,
            other => return Err(format!("미지 인자 {other:?}")),
        }
        i += 1;
    }

    // doc 입력: "-" 면 stdin(플러그인이 저작 산출을 파이프), 그 외 파일.
    let raw = if let Some(raw) = bundled {
        raw.as_bytes().to_vec()
    } else if path == "-" {
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut std::io::stdin(), &mut buf)
            .map_err(|e| format!("read stdin: {e}"))?;
        buf
    } else {
        std::fs::read(&path).map_err(|e| format!("read {path}: {e}"))?
    };
    let doc: Value =
        serde_json::from_slice(&raw).map_err(|e| format!("parse workflow-doc: {e}"))?;
    // 실행 입력은 선언된 workflow 문서 형식으로 닫혀 있다.
    if !soksak_sidecar_workflow::doc_interp::is_doc(&doc) {
        return Err("workflow-doc@0.0.1 필요(spec 필드)".to_string());
    }
    if !emit {
        return Err("workflow-doc 직접 실행 경로 없음(실행=스케줄러) — 발행은 --emit, stage 실행은 exec-stage".to_string());
    }
    let name = doc
        .pointer("/meta/name")
        .and_then(|n| n.as_str())
        .unwrap_or("workflow");
    if let Some(l) = &lang {
        eprintln!("[soksak] 출력 언어: {} ({})", l.name, l.code);
    }
    eprintln!("[soksak] {name} — 발행 모드(workflow-doc, 노드 DAG stdout, LLM 미호출)");
    // args = --args-json(verbatim) 우선, 없으면 --arg 조립. lang 주입.
    let mut args_json = args_override
        .take()
        .unwrap_or(Value::Object(std::mem::take(&mut args)));
    if let (Some(l), Value::Object(m)) = (&lang, &mut args_json) {
        m.insert("lang".to_string(), Value::String(l.code.clone()));
    }
    // skeleton stage("") — validate 가 agent op 를 금지하므로(발행=LLM 미호출 계약) 러너는 도달 불가 방어.
    let mut no_agent = |_p: &str, _s: Option<&Value>, _l: &str| -> Result<Value, String> {
        Err("발행(--emit)은 agent 를 호출하지 않는다".to_string())
    };
    let (events, _result) =
        soksak_sidecar_workflow::doc_interp::run(&doc, "", &args_json, &mut no_agent)?;
    for ev in &events {
        if let Ok(s) = serde_json::to_string(ev) {
            println!("{s}");
        }
    }
    Ok(())
}

/// run_build_ledger — build-ledger 서브커맨드(LLM 0). stdin {nodes:[...]} + --chunk <id> --kind <kind>
/// → 원장 JSON 배열(stdout). reconcile::build_ledger 의 CLI 미러 — app-0 러너가 JS 재구현 없이 소비한다.
fn run_build_ledger(argv: &[String]) -> Result<(), String> {
    use soksak_sidecar_workflow::reconcile::{build_ledger, Node};
    let mut chunk = String::new();
    let mut kind = String::new();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--chunk" => {
                i += 1;
                chunk = argv.get(i).cloned().ok_or("--chunk 값 누락")?;
            }
            "--kind" => {
                i += 1;
                kind = argv.get(i).cloned().ok_or("--kind 값 누락")?;
            }
            other => return Err(format!("build-ledger: 미지 인자 {other:?}")),
        }
        i += 1;
    }
    if chunk.is_empty() || kind.is_empty() {
        return Err("build-ledger: --chunk·--kind 필수".to_string());
    }
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)
        .map_err(|e| format!("read stdin: {e}"))?;
    let input: Value = serde_json::from_str(&raw).map_err(|e| format!("stdin JSON: {e}"))?;
    let nodes: Vec<Node> = serde_json::from_value(
        input
            .get("nodes")
            .cloned()
            .unwrap_or_else(|| Value::Array(vec![])),
    )
    .map_err(|e| format!("nodes 파싱: {e}"))?;
    let ledger = build_ledger(&nodes, &chunk, &kind);
    println!(
        "{}",
        serde_json::to_string(&ledger).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// run_exec_one — exec-one 서브커맨드. stdin {prompt, schema?, model?} 한 노드 → claude → {oxf, result}.
fn run_exec_one(argv: &[String]) -> Result<(), String> {
    let mut lang: Option<Language> = None;
    let mut allow_tools: Vec<String> = vec![];
    let mut model_override: Option<String> = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--lang" => {
                i += 1;
                lang = Some(Language::parse(argv.get(i).ok_or("--lang 값 누락")?));
            }
            "--allow-tools" => {
                i += 1;
                let t = argv.get(i).ok_or("--allow-tools 값 누락")?;
                allow_tools = t.split_whitespace().map(|s| s.to_string()).collect();
            }
            "--model" => {
                i += 1;
                model_override = Some(argv.get(i).ok_or("--model 값 누락")?.clone());
            }
            other => return Err(format!("exec-one: 미지 인자 {other:?}")),
        }
        i += 1;
    }
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)
        .map_err(|e| format!("read stdin: {e}"))?;
    let input = exec_one::parse_input(&raw)?;
    let model = model_override
        .or(input.model)
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());

    let (env, profile) = auth_env()?;
    // effort = 노드가 실은 tier, 미지정이면 최고(품질우선 — under-fund 방지). 로그로 tier 관통 관측 가능.
    let effort = input
        .effort
        .clone()
        .unwrap_or_else(|| soksak_sidecar_workflow::provider::DEFAULT_EFFORT.to_string());
    eprintln!(
        "[soksak] exec-one (model={model}, effort={effort}, 프로필={profile}) → {}",
        soksak_sidecar_workflow::provider::provider_label()
    );
    let full = build_prompt_with_schema(&input.prompt, None, lang.as_ref()); // schema 는 --json-schema 강제로(prompt X)
                                                                             // 7200s(2h): provider 캡 = claude 무한 방지용. lease=프로세스-생존이라 천장 통일 불필요 — 정상은 provider 가
                                                                             // claude 종료→onExit→reply(검색 fan-out 1h+ 수용). register timeout_ms(zombie_backstop 3h)는 그것도 실패한
                                                                             // 좀비 전용(provider 캡보다 길게). 중복은 lease(도는 중 재발화 X)로 0 — 천장 일치 안 해도 안전.
    let has_schema = input.schema.is_some();
    let req = AgentRequest {
        prompt: full,
        model: &model,
        allowed_tools: allow_tools,
        timeout_secs: 7200,
        system_prompt: None,
        schema: input.schema,
        effort,
        text_only: false,
    };
    // schema 있으면 JSON 파싱(구조화 산출), 없으면 raw 텍스트.
    let result = if has_schema {
        run_agent(&req, &env)?
    } else {
        Value::String(run_agent_text(&req, &env)?)
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&exec_one::build_output(result)).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// run_exec_stage — stage 작업 실행(모델 B). stdin {skeleton:{program}, stage, args, model?} → ClaudeEmitHost 해석.
/// opts.publish 없는 agent(예: genPrompt)는 claude 실행, opts.publish:true agent 는 자식 노드 발행(stdout JSON line).
/// 최종 워크플로 return 은 {ev:result, value} 로. 서비스 reconcile 이 kind=task 노드를 이걸로 실행해 동적 발행.
fn run_exec_stage(argv: &[String]) -> Result<(), String> {
    let mut lang: Option<Language> = None;
    let mut allow_tools: Vec<String> = vec![];
    let mut model_override: Option<String> = None;
    let mut assemble = false; // --assemble: LLM 0 — agent 턴의 {prompt, schema} 패키지만 stdout(pull 실행자용)
    let mut with_output: Option<Value> = None; // --with-output: LLM 0 — 외부 실행자 산출을 agent 턴에 주입해 발행 시퀀스 재생
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--lang" => {
                i += 1;
                lang = Some(Language::parse(argv.get(i).ok_or("--lang 값 누락")?));
            }
            "--allow-tools" => {
                i += 1;
                let t = argv.get(i).ok_or("--allow-tools 값 누락")?;
                allow_tools = t.split_whitespace().map(|s| s.to_string()).collect();
            }
            "--model" => {
                i += 1;
                model_override = Some(argv.get(i).ok_or("--model 값 누락")?.clone());
            }
            "--assemble" => assemble = true,
            "--with-output" => {
                i += 1;
                let raw = argv.get(i).ok_or("--with-output 값(JSON) 누락")?;
                with_output = Some(
                    serde_json::from_str(raw)
                        .map_err(|e| format!("--with-output JSON 파싱: {e}"))?,
                );
            }
            other => return Err(format!("exec-stage: 미지 인자 {other:?}")),
        }
        i += 1;
    }
    let mut raw = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw)
        .map_err(|e| format!("read stdin: {e}"))?;
    let input: Value =
        serde_json::from_str(raw.trim()).map_err(|e| format!("exec-stage 입력 JSON 파싱: {e}"))?;
    let stage = input
        .get("stage")
        .and_then(|s| s.as_str())
        .ok_or("exec-stage 입력에 stage 필요")?
        .to_string();
    // workflow 슬롯(이름) — 번들 정본 doc 로드(canonical doc 을 task 마다 임베드하지 않는 통로; 단일 원천=번들 파일).
    if let Some(name) = input.get("workflow").and_then(|w| w.as_str()) {
        let raw = bundled_workflow(name)?;
        let doc: Value =
            serde_json::from_str(raw).map_err(|e| format!("번들 워크플로 파싱 {name}: {e}"))?;
        if !soksak_sidecar_workflow::doc_interp::is_doc(&doc) {
            return Err(format!("번들 워크플로 {name:?} 가 workflow-doc@0.0.1 아님"));
        }
        return run_exec_stage_doc(
            &doc,
            &stage,
            &input,
            lang,
            allow_tools,
            model_override,
            assemble,
            with_output.clone(),
        );
    }
    // task body의 skeleton 슬롯도 동일한 workflow-doc 계약으로 검증한다.
    if let Some(doc) = input
        .get("skeleton")
        .filter(|s| soksak_sidecar_workflow::doc_interp::is_doc(s))
        .cloned()
    {
        return run_exec_stage_doc(
            &doc,
            &stage,
            &input,
            lang,
            allow_tools,
            model_override,
            assemble,
            with_output.clone(),
        );
    }
    Err("exec-stage 입력에 workflow(번들 이름) 또는 skeleton(workflow-doc@0.0.1) 필요".to_string())
}

/// run_exec_stage_doc — workflow-doc@0.0.1 stage 실행:
/// 산출 = {ev:add} 스트림 + {ev:result}(스테이지별 특수 포장 없음).
fn run_exec_stage_doc(
    doc: &Value,
    stage: &str,
    input: &Value,
    lang: Option<Language>,
    allow_tools: Vec<String>,
    model_override: Option<String>,
    assemble: bool,
    with_output: Option<Value>,
) -> Result<(), String> {
    // args = input.args + stage + lang 주입(interp 경로와 동일 조립 — 워크플로가 args.ledger/chunkRef 를 읽는다).
    let mut args_obj = match input.get("args") {
        Some(Value::Object(m)) => m.clone(),
        _ => Map::new(),
    };
    args_obj.insert("stage".to_string(), Value::String(stage.to_string()));
    if let Some(l) = &lang {
        args_obj.insert("lang".to_string(), Value::String(l.code.clone()));
    }
    let args_json = Value::Object(args_obj);
    // --assemble: agent 턴의 {prompt, schema} 패키지만 산출(LLM 0, 인증 불요) — pull 실행자가 자기 턴에 수행.
    if assemble {
        let mut captured: Option<Value> = None;
        let mut cap_fn = |prompt: &str,
                          schema: Option<&Value>,
                          label: &str|
         -> Result<Value, String> {
            captured = Some(
                serde_json::json!({ "prompt": build_prompt_with_schema(prompt, None, lang.as_ref()), "schema": schema, "label": label }),
            );
            Err("__ASSEMBLE_STOP__".into())
        };
        match soksak_sidecar_workflow::doc_interp::run(
            doc,
            stage,
            &{
                let mut a = match input.get("args") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                };
                a.insert("stage".to_string(), Value::String(stage.to_string()));
                if let Some(l) = &lang {
                    a.insert("lang".to_string(), Value::String(l.code.clone()));
                }
                Value::Object(a)
            },
            &mut cap_fn,
        ) {
            Ok(_) => {
                return Err(format!(
                    "--assemble: stage {stage:?} 에 agent 턴이 없다(패키지화 대상 아님)"
                ))
            }
            Err(e) if e.contains("__ASSEMBLE_STOP__") => {
                let pkg = captured.ok_or("--assemble: 캡처 실패")?;
                println!(
                    "{}",
                    serde_json::to_string(&pkg).map_err(|e| e.to_string())?
                );
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    }
    // --with-output: 외부 실행자(TUI 에이전트)의 산출을 agent 턴에 주입 — 발행 시퀀스만 재생(LLM 0, 인증 불요).
    if let Some(out) = with_output {
        let mut used = false;
        let mut inj_fn = |_prompt: &str,
                          schema: Option<&Value>,
                          label: &str|
         -> Result<Value, String> {
            if used {
                return Err(format!("--with-output: stage 에 agent 턴이 2개 이상({label:?}) — 주입 모드는 단일 턴 전제"));
            }
            used = true;
            if let Some(sc) = schema {
                check_required_keys(sc, &out)
                    .map_err(|e| format!("--with-output 산출이 schema 위반: {e}"))?;
            }
            Ok(out.clone())
        };
        let mut args_obj = match input.get("args") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        args_obj.insert("stage".to_string(), Value::String(stage.to_string()));
        if let Some(l) = &lang {
            args_obj.insert("lang".to_string(), Value::String(l.code.clone()));
        }
        let (events, result) = soksak_sidecar_workflow::doc_interp::run(
            doc,
            stage,
            &Value::Object(args_obj),
            &mut inj_fn,
        )?;
        return emit_stage_output(stage, events, result);
    }
    let model = model_override
        .or_else(|| {
            input
                .get("model")
                .and_then(|m| m.as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    // effort = stage 입력의 tier(저작 LLM 이 난이도로), 미지정이면 최고(품질우선).
    let effort = input
        .get("effort")
        .and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| soksak_sidecar_workflow::provider::DEFAULT_EFFORT.to_string());
    let (env, profile) = auth_env()?;
    eprintln!("[soksak] exec-stage stage={stage} (workflow-doc, model={model}, effort={effort}, 프로필={profile})");
    // agent 러너 — ClaudeHost.agent 동형(빌드 프롬프트+언어 계약, schema 는 --json-schema 강제, 실패는 전파).
    let mut agent_fn =
        |prompt: &str, schema: Option<&Value>, label: &str| -> Result<Value, String> {
            let full = build_prompt_with_schema(prompt, None, lang.as_ref());
            eprintln!(
                "[soksak] agent {label:?} (model={model}, effort={effort}) → {}",
                soksak_sidecar_workflow::provider::provider_label()
            );
            let req = AgentRequest {
                prompt: full,
                model: &model,
                allowed_tools: allow_tools.clone(),
                timeout_secs: 3600,
                system_prompt: None,
                schema: schema.cloned(),
                effort: effort.clone(),
                text_only: false,
            };
            if schema.is_some() {
                run_agent(&req, &env).map_err(|e| format!("agent {label:?} 실패: {e}"))
            } else {
                run_agent_text(&req, &env)
                    .map(Value::String)
                    .map_err(|e| format!("agent {label:?} 실패: {e}"))
            }
        };
    let (events, result) =
        soksak_sidecar_workflow::doc_interp::run(doc, stage, &args_json, &mut agent_fn)?;
    emit_stage_output(stage, events, result)
}

/// emit_stage_output — stage 실행 산출의 stdout 계약(정본 LLM 경로와 --with-output 재생이 공유).
/// NodeEvent 라인들 + result 라인. 스테이지별 특수 포장은 없다.
fn emit_stage_output(
    _stage: &str,
    events: Vec<soksak_sidecar_workflow::node_event::NodeEvent>,
    result: Value,
) -> Result<(), String> {
    for ev in &events {
        if let Ok(s) = serde_json::to_string(ev) {
            println!("{s}");
        }
    }
    let out = json!({ "ev": "result", "value": result });
    println!(
        "{}",
        serde_json::to_string(&out).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// check_required_keys — --with-output 산출의 최소 스키마 검증(required 재귀). 전체 검증은
/// 하류 게이트(draft validate·badge 파이프) 몫 — 여기선 필수 키 부재를 fail-loud 로만 막는다.

#[cfg(test)]
mod tests_pull_modes {
    use super::*;

    #[test]
    fn required_keys_checked_recursively() {
        let schema: Value = serde_json::json!({
            "type": "object", "required": ["facts"],
            "properties": { "facts": { "type": "array", "items": { "type": "object", "required": ["title"], "properties": { "title": {"type": "string"} } } } }
        });
        assert!(
            check_required_keys(&schema, &serde_json::json!({"facts": [{"title": "a"}]})).is_ok()
        );
        assert!(
            check_required_keys(&schema, &serde_json::json!({})).is_err(),
            "톱레벨 required 부재"
        );
        assert!(
            check_required_keys(&schema, &serde_json::json!({"facts": [{}]})).is_err(),
            "항목 required 부재"
        );
    }
}

fn check_required_keys(schema: &Value, value: &Value) -> Result<(), String> {
    if let Some(req) = schema.get("required").and_then(|r| r.as_array()) {
        for k in req {
            if let Some(key) = k.as_str() {
                if value.get(key).is_none() {
                    return Err(format!("required 키 부재: {key:?}"));
                }
            }
        }
    }
    if let (Some(props), Some(obj)) = (
        schema.get("properties").and_then(|p| p.as_object()),
        value.as_object(),
    ) {
        for (k, sub) in props {
            if let Some(v) = obj.get(k) {
                if !v.is_null() {
                    check_required_keys(sub, v)?;
                }
            }
        }
    }
    if let (Some(items), Some(arr)) = (schema.get("items"), value.as_array()) {
        for v in arr {
            check_required_keys(items, v)?;
        }
    }
    Ok(())
}

/// run_generate_skeleton — generate-skeleton 서브커맨드. 아이디어 → workflow-doc@0.0.1(LLM 저작) → doc JSON stdout.
/// system=SKILL+api+patterns+draft-skill(바이너리 포함, --refs 로 override 가능), user=아이디어+③파생.
/// 저작 게이트 = JSON 파싱(parse_json_lenient — 펜스/prose 방어) + doc_interp::validate(fail-loud).
// draft-run — idea → 인메모리 보드 DRAFT 흐름 완주. 실 provider 라 인증 env(ANTHROPIC_*/OAuth) 필수.
fn run_draft_run(argv: &[String]) -> Result<(), String> {
    let mut idea = String::new();
    let mut model: Option<String> = None;
    let mut lang = "ko".to_string();
    let mut dump: Option<String> = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--idea" => {
                i += 1;
                idea = argv.get(i).cloned().ok_or("--idea 값 누락")?;
            }
            "--model" => {
                i += 1;
                model = Some(argv.get(i).cloned().ok_or("--model 값 누락")?);
            }
            "--lang" => {
                i += 1;
                lang = argv.get(i).cloned().ok_or("--lang 값 누락")?;
            }
            "--dump" => {
                i += 1;
                dump = Some(argv.get(i).cloned().ok_or("--dump 값 누락")?);
            }
            other => return Err(format!("draft-run: 미지 인자 {other:?}")),
        }
        i += 1;
    }
    if idea.trim().is_empty() {
        return Err("draft-run: --idea 필수".to_string());
    }
    // 내용이 산출물이다 — 라운드별 요건 전문을 사람이 읽는 마크다운으로 남긴다. 미지정이면 cwd/draft-run.spec.md.
    let dump_path = dump.unwrap_or_else(|| "draft-run.spec.md".to_string());
    let report = soksak_sidecar_workflow::wf_service::draft_run(
        &idea,
        model.as_deref(),
        &lang,
        Some(std::path::Path::new(&dump_path)),
    )?;
    // 로그는 drive() 가 틱마다 라이브로 흘렸다 — 완주 시점엔 요약 + 덤프 경로만 찍는다(중복 방지).
    print!("{}", report.summary());
    eprintln!("[soksak] 요건 집합 전문: {dump_path}");
    Ok(())
}

fn run_generate_skeleton(argv: &[String]) -> Result<(), String> {
    let mut assemble = false; // --assemble: 정련 턴의 {prompt, schema} 패키지만(LLM 0)
    let mut with_refined: Option<Value> = None; // --with-refined: 외부 실행자의 정련 산출 주입(LLM 0)
    let mut idea = String::new();
    let mut model = DEFAULT_MODEL.to_string();
    let mut lang: Option<Language> = None;
    let mut gen_out: Option<String> = None;
    let mut refs: Option<String> = None;
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "--idea" => {
                i += 1;
                idea = argv.get(i).cloned().ok_or("--idea 값 누락")?;
            }
            "--model" => {
                i += 1;
                model = argv.get(i).cloned().ok_or("--model 값 누락")?;
            }
            "--lang" => {
                i += 1;
                lang = Some(Language::parse(argv.get(i).ok_or("--lang 값 누락")?));
            }
            "--gen-out" => {
                i += 1;
                gen_out = Some(argv.get(i).cloned().ok_or("--gen-out 값 누락")?);
            }
            "--refs" => {
                i += 1;
                refs = Some(argv.get(i).cloned().ok_or("--refs 값 누락")?);
            }
            "--assemble" => assemble = true,
            "--with-refined" => {
                i += 1;
                let raw = argv.get(i).ok_or("--with-refined 값(JSON) 누락")?;
                with_refined = Some(
                    serde_json::from_str(raw)
                        .map_err(|e| format!("--with-refined JSON 파싱: {e}"))?,
                );
            }
            other => return Err(format!("generate-skeleton: 미지 인자 {other:?}")),
        }
        i += 1;
    }
    if idea.trim().is_empty() {
        return Err("generate-skeleton: --idea 필수".to_string());
    }
    // 기본 정련 지침은 바이너리에 포함된다. --refs 는 사용자가 명시한 개발용 override 이다.
    let system = if let Some(refs_dir) = refs {
        std::fs::read_to_string(format!("{refs_dir}/draft-skill.md"))
            .map_err(|e| format!("refs 읽기 {refs_dir}/draft-skill.md: {e}"))?
    } else {
        soksak_sidecar_workflow::paths::draft_skill().to_string()
    };
    // system 층 = draft **정련** 역할 지시어. LLM 은 정련만 한다(PRINCIPLES §7) — 문서 골격·상수(COMMON·
    // 스키마·프롬프트)는 번들 정본(workflows/draft.doc.json)을 도구가 조립한다. 19KB verbatim 재타이핑은
    // 문자 하나 누락으로 전체가 깨지는 취약 구조였다(실측: 18,911번째 문자 따옴표 누락).
    // ③파생 도메인 지시어 → user 프롬프트 힌트.
    let directives = derive_directives(&idea, &builtin_library());
    let matched: Vec<&str> = directives
        .iter()
        .map(|d| d.domain.as_str())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    eprintln!(
        "[soksak] generate-skeleton: ③파생 도메인 {:?} → 지시어 {}개",
        matched,
        directives.len()
    );
    let mut user = build_user_prompt(&idea, &directives);
    if let Some(l) = &lang {
        user.push_str(&l.contract());
    }

    let refine_schema_v = json!({
        "type": "object",
        "required": ["directive", "description"],
        "properties": {
            "directive": { "type": "string", "description": "정련된 DIRECTIVE 전문 — 아이디어의 실제 의도를 담은 지시어(섹션 라벨 재구성 허용, 실질 요건 누락 금지)" },
            "description": { "type": "string", "description": "이 드래프트의 한 줄 서술(담백)" }
        }
    });
    // --assemble: 정련 턴 패키지만(LLM 0, 인증 불요) — pull 실행자가 자기 턴에 정련을 수행한다.
    if assemble {
        let pkg = json!({ "label": "refine", "prompt": format!("{system}\n\n{user}"), "schema": refine_schema_v });
        println!(
            "{}",
            serde_json::to_string(&pkg).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    // --with-refined: 외부 정련 산출 주입(LLM 0, 인증 불요) — 조립·검증 게이트는 동일.
    if let Some(out) = with_refined {
        let template: Value = serde_json::from_str(bundled_workflow("draft")?)
            .map_err(|e| format!("번들 draft 파싱: {e}"))?;
        let directive = out
            .get("directive")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let description = out
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if directive.is_empty() {
            return Err("--with-refined: directive 비어있음".into());
        }
        let doc = soksak_sidecar_workflow::doc_interp::inject_refinement(
            &template,
            &directive,
            &description,
        );
        if let Err(violations) = soksak_sidecar_workflow::doc_interp::validate(&doc) {
            return Err(format!(
                "조립 doc 검증 실패({}건): {}",
                violations.len(),
                violations.first().cloned().unwrap_or_default()
            ));
        }
        println!(
            "{}",
            serde_json::to_string(&doc).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    // LLM 정련 실경로 = lib 단일진실(author_doc::generate_doc). CLI 와 serve(wf_service) 가 같은
    // 함수를 부른다 — system=draft-skill.md, 골격 상수=번들 draft.doc.json 조립, 산출 {directive, description}
    // 소형 JSON(재타이핑 0), 정련 2회. assemble/with-refined(LLM 0)만 CLI 전용으로 위에서 처리했다.
    let (env, profile) = auth_env()?;
    eprintln!(
        "[soksak] generate-skeleton (model={model}, 프로필={profile}) → claude -p 정련(directive)"
    );
    let doc = soksak_sidecar_workflow::author_doc::generate_doc(
        &idea,
        &model,
        lang.as_ref(),
        &system,
        &env,
        gen_out.as_deref(),
    )?;
    println!(
        "{}",
        serde_json::to_string(&doc).map_err(|e| e.to_string())?
    );
    Ok(())
}

//! directive-loop — 독립 파이프라인 CLI.
//!   한 줄 지시 → 생성기능 → [검증기능 ↔ 결정기능] 루프 → 렌더기능 → 검증된 기능정의 스펙.
//!   정적 도메인 주입 없음. 주제 원장(JSON)에 누적·재사용. 누락은 검증기능 제안 → 결정기능 승격.
//!   **인증 프로필**: ANTHROPIC_* 를 부모 환경에서 export 해 호출(토큰 비하드코딩).
//!     예: ANTHROPIC_AUTH_TOKEN=… ANTHROPIC_BASE_URL=… ANTHROPIC_DEFAULT_OPUS_MODEL=glm-5.2 \
//!         directive-loop "산악회 금강산 등반 계획" --store /declared/path/climb.json --rounds 6

use soksak_sidecar_workflow::directive_loop::{run_loop, Ledger, LoopConfig};
use std::path::PathBuf;

/// 부모 환경에서 claude 프로필 인증/설정 변수만 수집 → run_agent 에 격리 주입.
/// OAuth 프로필: CLAUDE_ACCOUNT_NAME + CLAUDE_CODE_OAUTH_TOKEN.
/// 인증 env: ANTHROPIC_*. 부모에 export 된 것만 통과한다(어느 프로필이든).
fn profile_env() -> Vec<(String, String)> {
    [
        "CLAUDE_ACCOUNT_NAME",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "ANTHROPIC_ACCOUNT_NAME",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
    ]
    .iter()
    .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
    .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut directive = String::new();
    let mut store: Option<PathBuf> = None;
    let mut rounds: u32 = 6;
    let mut concurrency: usize = 5; // (A)검증 병렬 배치 수. 웹서치 버스트 시 줄임.
    let mut verifier_model = "sonnet".to_string(); // glm-5.1 — 빠른 집중 검증.
    let mut exec_model = "opus".to_string(); // glm-5.2 — broad 추론.
    let mut force = false; // 이미 수렴한 store 도 강제 재개.
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--store" => {
                i += 1;
                store = Some(PathBuf::from(args.get(i).cloned().unwrap_or_default()));
            }
            "--rounds" => {
                i += 1;
                rounds = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(6);
            }
            "--concurrency" => {
                i += 1;
                concurrency = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(5);
            }
            "--verifier-model" => {
                i += 1;
                verifier_model = args.get(i).cloned().unwrap_or(verifier_model);
            }
            "--exec-model" => {
                i += 1;
                exec_model = args.get(i).cloned().unwrap_or(exec_model);
            }
            "--force" => force = true,
            s if directive.is_empty() && !s.starts_with("--") => directive = s.to_string(),
            _ => {}
        }
        i += 1;
    }
    if directive.is_empty() {
        eprintln!("usage: directive-loop \"<지시>\" --store /absolute/path [--rounds N] [--verifier-model m] [--exec-model m]");
        std::process::exit(2);
    }
    let Some(store) = store else {
        eprintln!("directive-loop: --store requires an absolute path");
        std::process::exit(2);
    };
    if !store.is_absolute() {
        eprintln!("directive-loop: --store requires an absolute path");
        std::process::exit(2);
    }

    let prof = profile_env();
    if !prof
        .iter()
        .any(|(k, _)| k == "CLAUDE_CODE_OAUTH_TOKEN" || k == "ANTHROPIC_AUTH_TOKEN")
    {
        eprintln!("[directive-loop] 경고: 프로필 인증 토큰 미설정 — CLAUDE_CODE_OAUTH_TOKEN 또는 ANTHROPIC_AUTH_TOKEN export 필요");
    }
    let cfg = LoopConfig {
        agent_env: prof,
        verifier_model,
        exec_model,
        max_rounds: rounds,
        concurrency,
    };

    if let Some(parent) = store.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // advisory 파일 락이 원장의 단일 writer를 소유한다. 실행 중인 writer가 있으면 즉시 실패하며,
    // --force도 소유권을 빼앗지 않는다. 프로세스 종료 시 OS가 락을 해제하므로 stale 판정이나 폴링이 없다.
    let lock_path = {
        let mut p = store.clone().into_os_string();
        p.push(".lock");
        PathBuf::from(p)
    };
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap_or_else(|e| {
            eprintln!(
                "[directive-loop] 락 파일 열기 실패 {}: {e}",
                lock_path.display()
            );
            std::process::exit(2);
        });
    if let Err(e) = lock_file.try_lock() {
        match e {
            std::fs::TryLockError::WouldBlock => {
                eprintln!(
                    "[directive-loop] 다른 프로세스가 store 락을 소유함: {}",
                    store.display()
                );
                std::process::exit(2);
            }
            std::fs::TryLockError::Error(err) => {
                eprintln!(
                    "[directive-loop] 락 획득 오류 {}: {err}",
                    lock_path.display()
                );
                std::process::exit(2);
            }
        }
    }
    // lock_file은 main 끝까지 살아 있어야 락이 유지된다.

    if force {
        // 락을 소유한 상태에서만 기존 산출을 지우고 처음부터 시작한다.
        let _ = std::fs::remove_file(&store);
        let _ = std::fs::remove_file(store.with_extension("result.json"));
        eprintln!("[directive-loop] --force: 기존 ledger·result 삭제 — 처음부터 시작.");
    } else {
        // 다 돌았는지 — 이미 수렴한 store 는 재실행 생략(락="돌고있는지", 이건 "다 돌았는지").
        // 락 획득 후 검사(직렬화됨): 살아있는 writer 가 없을 때만 여기 도달한다.
        let result_path = store.with_extension("result.json");
        if let Ok(txt) = std::fs::read_to_string(&result_path) {
            if serde_json::from_str::<serde_json::Value>(&txt)
                .ok()
                .and_then(|v| v.get("converged").and_then(|b| b.as_bool()))
                == Some(true)
            {
                eprintln!(
                    "[directive-loop] 이미 수렴 완료(다 돌았음) — 재실행 생략: {}. 강제하려면 --force.",
                    store.display()
                );
                std::process::exit(0);
            }
        }
    }

    let mut ledger = Ledger::load(store.clone());
    eprintln!(
        "[directive-loop] 시작: {:?} (store={}, 기존 주제={}, 상한={})",
        directive,
        store.display(),
        ledger.topics.len(),
        rounds
    );

    match run_loop(&directive, &mut ledger, &cfg) {
        Ok(o) => {
            eprintln!(
                "[directive-loop] ── 결과: converged={} aborted={} rounds={} 주제={} ──",
                o.converged,
                o.aborted,
                o.rounds,
                o.topics.len()
            );
            for r in &o.log {
                eprintln!(
                    "  R{} [{}] 검증 ✓{} ✗{} 승격+{} — {}",
                    r.round,
                    r.decision,
                    r.verified,
                    r.failed,
                    r.promoted,
                    r.reason.chars().take(140).collect::<String>()
                );
            }
            if o.aborted {
                eprintln!("[directive-loop] ABORT: {}", o.abort_reason);
            }
            // 결과(Outcome JSON) → stdout + 결과 파일. 주제 원장은 store 에 누적.
            match serde_json::to_string_pretty(&o) {
                Ok(j) => {
                    let result_path = store.with_extension("result.json");
                    if let Err(e) = std::fs::write(&result_path, &j) {
                        eprintln!(
                            "[directive-loop] 결과 파일 쓰기 실패 {}: {e}",
                            result_path.display()
                        );
                    } else {
                        eprintln!("[directive-loop] 결과 파일: {}", result_path.display());
                    }
                    println!("{j}");
                }
                Err(e) => {
                    eprintln!("[directive-loop] 결과 직렬화 실패: {e}");
                    std::process::exit(1);
                }
            }
            if !o.converged {
                std::process::exit(3); // 미수렴/abort — 게이트 신호.
            }
        }
        Err(e) => {
            eprintln!("[directive-loop] 실패: {e}");
            std::process::exit(1);
        }
    }
}

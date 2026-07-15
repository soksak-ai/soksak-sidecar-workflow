//! provider — agent 러너. `claude -p`(headless Claude Code)로 위임한다.
//! 인증·모델은 주입된 env 프로필이 정한다. 검증된 호출:
//!   claude -p <prompt> --output-format json --allowedTools "<...>" --strict-mcp-config --model <m>
//! → 이벤트 배열의 result.result(텍스트)를 코드펜스 제거 후 JSON 파싱.
//! 코어(vsterm)가 실제로는 env/spawn 을 위임하지만, 런타임 e2e 는 직접 spawn 해 검증한다.

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// thinking 진행 heartbeat 카운터(프로세스별). 긴 thinking 동안 '멈춘 것처럼' 안 보이게 한다.
static THINK_BEAT: AtomicUsize = AtomicUsize::new(0);

/// AgentRequest — 한 agent 실행 입력.
pub struct AgentRequest<'a> {
    /// 완성된 프롬프트(directive + ${placeholder} 바인딩 + schema 지시).
    pub prompt: String,
    /// 모델 별칭(haiku/sonnet/opus) — 실제 모델은 인증 프로필이 매핑.
    pub model: &'a str,
    /// 허용 tool(기본 빈 = 순수 생성). 일부 agent(WebSearch 등)는 명시.
    pub allowed_tools: Vec<String>,
    /// claude 호출 하드캡(초). hung 호출이 영원히 안 막게 — 단 검색 fan-out 단일 턴이 30분 넘을 수 있어
    /// 넉넉히 3600s(1시간). exec-one 은 코어와 천장 일치: provider 캡=register timeout_ms=ipc 클램프=3600s.
    /// 셋 일치라야 발화 timeout 으로 lease 중복 실행이 안 난다.
    pub timeout_secs: u64,
    /// system prompt(claude `--append-system-prompt`). user prompt(`-p`)와 분리 —
    /// SKILL.md(AST 사용법) + cc 추출 AST(구조 예시) 등이 system 층에, derive 지시어+아이디어가 user 층.
    /// None 이면 종래 동작(system 주입 없음).
    pub system_prompt: Option<String>,
    /// JSON Schema(claude `--json-schema`) — StructuredOutput 강제. agent 가 schema 준수 객체 반환(필수 필드 보장).
    /// api-reference 계약(schema → forced StructuredOutput → validated object). None 이면 raw 텍스트.
    pub schema: Option<Value>,
    /// claude `--effort`(codex 는 `-c model_reasoning_effort` 로 매핑). 추론 깊이. 노드 tier 로 설정,
    /// 미지정 경로는 DEFAULT_EFFORT(max=최고, codex 매핑 ultra)로 폴백 — 품질우선.
    pub effort: String,
    /// true 면 **순수 텍스트 반환 계약** — 파일/실행/검색 도구를 전면 차단(--disallowedTools 로).
    /// generate-skeleton 저작에만 씀: 도구가 열려 있으면 모델이 파일을 쓰려다 실패할 수 있다(빈
    /// --allowedTools를 claude CLI가 무시함). false면 allowed_tools 정책을 따른다.
    pub text_only: bool,
}

/// claude_args — AgentRequest → claude CLI 인자 벡터(순수, 테스트 가능).
/// run_agent_text 가 timeout 래퍼로 이 args 를 `claude` 에 적용한다. system_prompt 가 Some 이면
/// `--append-system-prompt <내용>` 추가(user prompt 와 분리 — claude CLI 공식 플래그).
/// zai_token — z.ai API 키(= glm 인증 토큰 재사용, 별도 키 불요). 이름 관례가 여럿이라 순서대로 찾는다:
/// ZAI_API_KEY(사용자 셸) → Z_AI_API_KEY(@z_ai/mcp-server 문서 관례) → ANTHROPIC_AUTH_TOKEN(ccglm 사이드카).
fn zai_token() -> Option<String> {
    ["ZAI_API_KEY", "Z_AI_API_KEY", "ANTHROPIC_AUTH_TOKEN"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
}

/// is_zai_url — Anthropic-compat base URL 이 z.ai(glm) 프로필인가(순수). z.ai 는 Anthropic 의 native WebSearch
/// 를 지원하지 않으므로 검색을 z.ai MCP 로 돌리고, real Anthropic(claude)은 native WebSearch 를 쓴다.
fn is_zai_url(base_url: Option<&str>) -> bool {
    base_url
        .map(|u| {
            let u = u.to_ascii_lowercase();
            u.contains("z.ai") || u.contains("bigmodel") || u.contains("zhipu")
        })
        .unwrap_or(false)
}

/// is_zai_profile — 실행 프로필 판별(env). ccglm 등 z.ai 프로필은 ANTHROPIC_BASE_URL 로 구별된다.
fn is_zai_profile() -> bool {
    is_zai_url(std::env::var("ANTHROPIC_BASE_URL").ok().as_deref())
}

/// default_search_servers — 검색 substrate 기본 배선(순수, 테스트 가능). 규칙: 프롬프트가 요구하는 도구를
/// 실제로 노출한다고 프로브(tools/mcp-verify.mjs)로 검증된 서버만 배선한다. "노출할 것이다" 가정 배선 금지.
///   context7 — 라이브러리 docs·버전·호환(키 없음, 두 프로필 공용 — claude 도 native docs 도구는 없다).
///   웹검색 — z.ai 프로필일 때만 z.ai web_search_prime(정식 원격 API·유료 쿼터·실데이터 반환 확인). real
///            claude 는 native WebSearch 를 쓰므로 웹검색 MCP 를 배선하지 않는다(claude_args 가 WebSearch 허용).
///            z.ai 패키지 @z_ai/mcp-server 는 비전 전용(웹검색 0)이라 절대 배선 금지.
///   tavily/brave — 키 있을 때만 추가 소스(두 프로필 공용).
fn default_search_servers(
    zai: bool,
    zai_key: Option<&str>,
    tavily: Option<&str>,
    brave: Option<&str>,
) -> Value {
    let mut servers = serde_json::Map::new();
    servers.insert(
        "context7".into(),
        serde_json::json!({
        "type": "stdio", "command": "npx", "args": ["-y", "@upstash/context7-mcp"] }),
    );
    if zai {
        if let Some(k) = zai_key {
            servers.insert(
                "web-search-prime".into(),
                serde_json::json!({
                "type": "http",
                "url": "https://api.z.ai/api/mcp/web_search_prime/mcp",
                "headers": { "Authorization": format!("Bearer {k}") } }),
            );
        }
    }
    if let Some(k) = tavily {
        servers.insert("tavily".into(), serde_json::json!({
            "type": "stdio", "command": "npx", "args": ["-y", "tavily-mcp"], "env": { "TAVILY_API_KEY": k } }));
    }
    if let Some(k) = brave {
        servers.insert("brave".into(), serde_json::json!({
            "type": "stdio", "command": "npx", "args": ["-y", "@modelcontextprotocol/server-brave-search"], "env": { "BRAVE_API_KEY": k } }));
    }
    serde_json::json!({ "mcpServers": servers })
}

/// search_mcp — 발굴/판정 에이전트의 --mcp-config + allowedTools 토큰(mcp__<서버>) 생성. zai=프로필.
/// SOKSAK_WORKFLOW_MCP_CONFIG(파일 경로 또는 raw JSON)로 전면 override(서버명 파싱해 grant).
fn search_mcp(zai: bool) -> Option<(String, Vec<String>)> {
    let cfg: Value = match std::env::var("SOKSAK_WORKFLOW_MCP_CONFIG")
        .ok()
        .filter(|v| !v.trim().is_empty())
    {
        Some(v) => {
            let raw = if std::path::Path::new(&v).is_file() {
                std::fs::read_to_string(&v).ok()?
            } else {
                v
            };
            serde_json::from_str(&raw).ok()?
        }
        None => default_search_servers(
            zai,
            zai_token().as_deref(),
            std::env::var("TAVILY_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
                .as_deref(),
            std::env::var("BRAVE_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
                .as_deref(),
        ),
    };
    let tokens: Vec<String> = cfg
        .get("mcpServers")
        .and_then(|m| m.as_object())
        .map(|m| m.keys().map(|k| format!("mcp__{k}")).collect())
        .unwrap_or_default();
    if tokens.is_empty() {
        return None;
    }
    Some((cfg.to_string(), tokens))
}

/// claude_args — AgentRequest → claude CLI 인자 벡터. 실행 프로필로 도구 정책 분기(is_zai_profile).
fn claude_args(req: &AgentRequest) -> Vec<String> {
    claude_args_impl(req, is_zai_profile())
}

/// claude_args_impl — 순수(프로필 명시, 테스트 가능). 도구 정책:
///   text_only(저작): 파일·실행·검색 전면 차단(순수 텍스트만 반환).
///   z.ai/glm: Anthropic WebSearch 미지원(요청 시 -p grant hang) → 차단, z.ai web_search_prime+context7 로 검색.
///   real claude: native WebSearch 사용(차단 안 하고 명시 grant) + context7 보강. 두 경우 파일·셸 배회는 차단
///                (Bash 를 안 막으면 git/ls 로 배회하다 대형 출력에서 타임아웃 — 실측).
fn claude_args_impl(req: &AgentRequest, zai: bool) -> Vec<String> {
    const WANDER: &str =
        "Task Bash Read Write Edit MultiEdit Glob Grep NotebookEdit TodoWrite WebFetch";
    let (disallowed, mcp, grant_websearch): (String, Option<(String, Vec<String>)>, bool) =
        if req.text_only {
            (format!("{WANDER} WebSearch"), None, false)
        } else if zai {
            (format!("{WANDER} WebSearch"), search_mcp(true), false)
        } else {
            (WANDER.to_string(), search_mcp(false), true)
        };
    // allowedTools = req 명시분 + 배선 MCP 도구 + (real claude) native WebSearch — -p 에서 권한 프롬프트 없이
    // 즉시 사용(위험 플래그 없이). StructuredOutput 은 --json-schema 강제라 영향 없음.
    let mut allowed = req.allowed_tools.clone();
    if let Some((_, ref tokens)) = mcp {
        allowed.extend(tokens.iter().cloned());
    }
    if grant_websearch {
        allowed.push("WebSearch".into());
    }
    let mut args: Vec<String> = vec![
        "-p".into(),
        req.prompt.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--strict-mcp-config".into(),
        "--allowedTools".into(),
        allowed.join(" "),
        "--disallowedTools".into(),
        disallowed,
        "--model".into(),
        req.model.into(),
    ];
    if let Some((cfg, _)) = mcp {
        args.push("--mcp-config".into());
        args.push(cfg);
    }
    if let Some(sp) = &req.system_prompt {
        args.push("--append-system-prompt".into());
        args.push(sp.clone());
    }
    if let Some(sc) = &req.schema {
        args.push("--json-schema".into());
        args.push(sc.to_string());
    }
    args.push("--effort".into());
    args.push(req.effort.clone());
    args
}

/// run_agent_text — claude -p 로 agent 실행, result 텍스트(raw) 반환. **529 과부하 재시도** 포함.
/// 제공자 529 과부하는 흔하고 일시적 → **고정 30초 간격 재실행**(사용자 확정 정책 — 지수 backoff 아님).
/// max 10회(과부하 ~5분 창 커버). 최종 실패는 loud.
pub fn run_agent_text(req: &AgentRequest, env: &[(String, String)]) -> Result<String, String> {
    const MAX: u32 = 10;
    const INTERVAL_SECS: u64 = 30;
    for attempt in 0..MAX {
        match run_agent_text_once(req, env) {
            Ok(s) => return Ok(s),
            Err(e) if is_529(&e) && attempt + 1 < MAX => {
                eprintln!(
                    "[soksak] 529 과부하 — {INTERVAL_SECS}s 후 재실행 ({}/{MAX})",
                    attempt + 1
                );
                std::thread::sleep(std::time::Duration::from_secs(INTERVAL_SECS));
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!()
}

/// runs_dir — run catalog 위치 해석(상시 경로 = identity 홈, 진단 override = 사이드카 소유 env).
/// ① SOKSAK_SIDECAR_WORKFLOW_RUNS(진단 — SIDECARS.md 의 SOKSAK_SIDECAR_{NAME}_* 채널)
/// ② $SOKSAK_HOME/runs/soksak-sidecar-workflow(앱 주입 컨텍스트 — A17)
/// ③ 없으면 None(기록 비활성 — 독립 CLI 는 하네스가 ① 로 지정한다).
fn runs_dir() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("SOKSAK_SIDECAR_WORKFLOW_RUNS") {
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    if let Ok(h) = std::env::var("SOKSAK_HOME") {
        if !h.is_empty() {
            return Some(
                std::path::PathBuf::from(h)
                    .join("runs")
                    .join("soksak-sidecar-workflow"),
            );
        }
    }
    None
}

fn ensure_regular_directory(path: &std::path::Path) -> std::io::Result<()> {
    use std::path::Component;
    let mut cursor = std::path::PathBuf::new();
    for component in path.components() {
        cursor.push(component.as_os_str());
        // Prefixes (e.g. a Windows `\\?\C:` verbatim drive) and the filesystem
        // root are structural anchors, not directories to validate or create;
        // statting them on their own is meaningless and errors on Windows.
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        match std::fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("regular directory required: {}", cursor.display()),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&cursor)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

static RUN_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Create one immutable run stream and replace `latest.json` with a regular JSON
/// pointer. The catalog never aliases files; readers resolve the declared
/// `stream` field under the configured run directory.
fn create_run_stream(dir: &std::path::Path) -> Option<std::fs::File> {
    use std::io::Write;
    use std::sync::atomic::Ordering;

    ensure_regular_directory(dir).ok()?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = format!("{ts}-{}-{sequence}.jsonl", std::process::id());
    let path = dir.join(&name);
    let stream = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .ok()?;

    let pointer_name = format!(".latest-{}-{sequence}.tmp", std::process::id());
    let pointer_path = dir.join(&pointer_name);
    let mut pointer = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pointer_path)
        .ok()?;
    if writeln!(pointer, "{}", serde_json::json!({ "stream": name })).is_err()
        || pointer.sync_all().is_err()
    {
        let _ = std::fs::remove_file(&pointer_path);
        return None;
    }
    drop(pointer);
    let latest = dir.join("latest.json");
    #[cfg(windows)]
    if latest.exists() {
        std::fs::remove_file(&latest).ok()?;
    }
    if std::fs::rename(&pointer_path, &latest).is_err() {
        let _ = std::fs::remove_file(&pointer_path);
        return None;
    }
    eprintln!("[soksak] run stream → {}", path.display());
    Some(stream)
}

fn open_run_stream() -> Option<std::fs::File> {
    create_run_stream(&runs_dir()?)
}

/// event_signals_529 — stream 이벤트에서 일시 과부하 신호 감지(순수). 텍스트는 톱레벨
/// {type:"text"} 가 아니라 **assistant 이벤트의 message.content[] text 블록**으로 온다(실측:
/// "API Error: 529 …" 가 assistant 블록 — 톱레벨만 보던 감지는 죽은 조건이라 재시도가 0이었다).
fn event_signals_529(ev: &serde_json::Value) -> bool {
    let ty = ev.get("type").and_then(|x| x.as_str()).unwrap_or("");
    // 구조 신호 최우선 — result 이벤트가 api_error_status 를 명시(run catalog 실측: 529).
    if ty == "result" {
        return ev
            .get("api_error_status")
            .and_then(|x| x.as_u64())
            .is_some_and(|c| (500..600).contains(&c));
    }
    if ty == "text" {
        return ev.get("text").and_then(|x| x.as_str()).is_some_and(is_529);
    }
    if ty == "assistant" || ty == "user" {
        if let Some(blocks) = ev
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            return blocks.iter().any(|b| {
                b.get("type").and_then(|t| t.as_str()) == Some("text")
                    && b.get("text").and_then(|t| t.as_str()).is_some_and(is_529)
            });
        }
    }
    false
}

/// is_529 — 제공자 일시 과부하 에러 판정. "wait longer"는 과부하 안내 문구(앱 실측 2026-07-03).
fn is_529(err: &str) -> bool {
    // transient 사전(§15): API 혼잡 + 연결단절 부류 — 재시도 대상. 결정적 실패는 여기 넣지 않는다.
    let e = err.to_ascii_lowercase();
    [
        "529",
        "overloaded",
        "temporarily",
        "wait longer",
        "econnreset",
        "econnrefused",
        "unable to connect",
        "socket hang up",
        "connection closed",
        "429",
        "rate limit",
        "usage limit",
    ]
    .iter()
    .any(|p| e.contains(p))
}

/// run_agent_text_once — claude -p 단일 실행(재시도 없음). 529 감지(stream text) 시 Err 를 529 로.
/// timeout 하드캡(req.timeout_secs)은 **네이티브**(wait-timeout crate) — 외부 GNU `timeout` 바이너리에
/// 의존하지 않는다(macOS 기본 미탑재; 부재 시 모든 호출이 "spawn claude" 오진 라벨로 죽던 결함 해소).
/// provider_kind — 실행 LLM CLI 선택. env SOKSAK_WORKFLOW_PROVIDER=codex 면 codex exec 어댑터,
/// 그 외(기본) claude -p. doc·보드·badge 파이프는 실행자 중립 — 여기 한 곳만 갈린다.
fn provider_kind() -> &'static str {
    match std::env::var("SOKSAK_WORKFLOW_PROVIDER").ok().as_deref() {
        Some("codex") => "codex",
        _ => "claude",
    }
}

/// provider_label — 실행자 CLI 표기(로그용). exec-one/exec-stage 가 "→ claude -p" 를 하드코딩하면
/// codex 경로에서 오표기 — 실행자를 실제로 반영한다.
pub fn provider_label() -> &'static str {
    match provider_kind() {
        "codex" => "codex exec",
        _ => "claude -p",
    }
}

/// normalize_schema_for_openai — OpenAI strict structured-output 방언으로 결정적 정규화(의미 보존):
/// 모든 object 에 additionalProperties=false, properties 전 키를 required 로(원래 선택이던 키는
/// type 에 "null" 을 더해 nullable 로 — 선택성의 등가 표현). Anthropic 스키마는 관대해 이 변환의
/// 역은 불필요. 재귀(중첩 object/array items).
fn normalize_schema_for_openai(v: &mut Value) {
    match v {
        Value::Object(m) => {
            let is_object_schema = m.get("type").and_then(|t| t.as_str()) == Some("object")
                || m.contains_key("properties");
            if is_object_schema {
                m.entry("additionalProperties")
                    .or_insert(Value::Bool(false));
                let prop_keys: Vec<String> = m
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|p| p.keys().cloned().collect())
                    .unwrap_or_default();
                let required: Vec<String> = m
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let optional: Vec<String> = prop_keys
                    .iter()
                    .filter(|k| !required.contains(k))
                    .cloned()
                    .collect();
                if let Some(props) = m.get_mut("properties").and_then(|p| p.as_object_mut()) {
                    for k in &optional {
                        if let Some(ps) = props.get_mut(k).and_then(|x| x.as_object_mut()) {
                            match ps.get_mut("type") {
                                Some(Value::String(t)) => {
                                    let t2 = t.clone();
                                    ps.insert("type".into(), serde_json::json!([t2, "null"]));
                                }
                                Some(Value::Array(a)) => {
                                    if !a.iter().any(|x| x.as_str() == Some("null")) {
                                        a.push(Value::String("null".into()));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                if !prop_keys.is_empty() {
                    m.insert(
                        "required".into(),
                        Value::Array(prop_keys.into_iter().map(Value::String).collect()),
                    );
                }
            }
            for (_, child) in m.iter_mut() {
                normalize_schema_for_openai(child);
            }
        }
        Value::Array(a) => {
            for child in a.iter_mut() {
                normalize_schema_for_openai(child);
            }
        }
        _ => {}
    }
}

/// run_codex_once — codex exec 어댑터(claude -p 등가): 프롬프트=stdin, 스키마=--output-schema(파일),
/// 스트림=--json(run catalog 보존), 결과=-o(최종 메시지 파일). 인증은 codex 자체 로그인(~/.codex) —
/// ANTHROPIC env 불요. 하드캡·transient 계약은 claude 경로와 동일.
/// 기본 reasoning effort — 미지정 시 실행자가 쓰는 값. **품질우선: 최고(claude `max`, codex 매핑 `ultra`)**.
/// 라우팅은 저작 LLM 이 노드에 명시적으로 더 낮은 tier 를 실을 때만 하향한다(under-fund 방지 — 기본은 최고).
pub const DEFAULT_EFFORT: &str = "max";

/// codex_reasoning_effort — 추상 effort(우리 어휘, claude `--effort` 기준: low/medium/high/xhigh/max)를
/// codex `model_reasoning_effort` 값으로 매핑. 두 provider 최고 tier 가 다름(claude `max` ↔ codex `ultra`)이라
/// 최고만 정렬하고 나머지는 codex 도 수용하는 동명값(low/medium/high/xhigh)을 그대로 넘긴다. codex 는
/// minimal/none 도 있으나 우리 어휘엔 없어 매핑 대상 아님(미지정 시 codex config 기본).
fn codex_reasoning_effort(effort: &str) -> &str {
    match effort {
        "max" => "ultra", // 각 provider 최고를 정렬
        other => other,   // low/medium/high/xhigh — codex 동명 수용
    }
}

fn run_codex_once(req: &AgentRequest) -> Result<String, String> {
    let tmp = std::env::temp_dir().join(format!("soksak-codex-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| format!("codex tmp: {e}"))?;
    let out_file = tmp.join(format!(
        "last-{}.txt",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-lc", r#"exec codex "$@""#, "codex"]);
    cmd.arg("exec")
        .arg("--json")
        .arg("--skip-git-repo-check")
        .arg("--ephemeral");
    cmd.arg("-o").arg(&out_file);
    if !req.model.is_empty() && req.model != "default" {
        // "default" = codex 자체 기본 모델(config) 사용 — -m 생략.
        cmd.arg("-m").arg(req.model);
    }
    // reasoning effort 배선(파리티) — claude `--effort` 와 대칭으로 codex 도 명시 전달한다. codex 는
    // `-c model_reasoning_effort=<v>` config override(STEP 0 실측: CLI 미검증). effort 어휘가 provider
    // 마다 달라 최고를 정렬한다(claude `max` ↔ codex `ultra`). 미지정("")이면 codex config 기본에 맡김.
    if !req.effort.is_empty() {
        let mapped = codex_reasoning_effort(&req.effort);
        let m = if req.model.is_empty() || req.model == "default" {
            "default"
        } else {
            req.model
        };
        eprintln!(
            "[soksak] codex exec (model={m}, effort={}→{mapped}) via -c model_reasoning_effort",
            req.effort
        );
        cmd.arg("-c")
            .arg(format!("model_reasoning_effort={mapped}"));
    }
    let schema_file = if let Some(sc) = &req.schema {
        let f = tmp.join(format!("schema-{}.json", std::process::id()));
        let mut sc2 = sc.clone();
        normalize_schema_for_openai(&mut sc2);
        std::fs::write(&f, sc2.to_string()).map_err(|e| format!("codex schema 기록: {e}"))?;
        cmd.arg("--output-schema").arg(&f);
        Some(f)
    } else {
        None
    };
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn codex: {e}"))?;
    {
        use std::io::Write;
        let mut si = child.stdin.take().ok_or("codex stdin 없음")?;
        // 프롬프트는 stdin — 인자 길이 한계 회피(수십 KB 프롬프트).
        let mut full = String::new();
        if let Some(sp) = &req.system_prompt {
            full.push_str(sp);
            full.push_str("\n\n");
        }
        full.push_str(&req.prompt);
        si.write_all(full.as_bytes())
            .map_err(|e| format!("codex stdin 쓰기: {e}"))?;
    } // drop = EOF
    let stdout = child.stdout.take().ok_or("codex stdout 없음")?;
    let reader = std::thread::spawn(move || {
        let mut run_stream = open_run_stream();
        let mut tail: Vec<String> = Vec::new(); // 실패 진단용 꼬리(transient 사전 매칭 재료)
        for line in BufReader::new(stdout).lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(f) = run_stream.as_mut() {
                use std::io::Write;
                let _ = writeln!(f, "{t}");
            }
            eprintln!("  [codex] {}", t.chars().take(160).collect::<String>());
            tail.push(t.chars().take(300).collect());
            if tail.len() > 8 {
                tail.remove(0);
            }
        }
        tail.join(" | ")
    });
    use wait_timeout::ChildExt;
    let status = match child
        .wait_timeout(std::time::Duration::from_secs(req.timeout_secs))
        .map_err(|e| format!("wait codex: {e}"))?
    {
        Some(st) => st,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(format!("codex 타임아웃({}s) — 강제 종료", req.timeout_secs));
        }
    };
    let tail = reader
        .join()
        .map_err(|_| "codex stream 리더 panic".to_string())?;
    if let Some(f) = schema_file {
        let _ = std::fs::remove_file(f);
    }
    if !status.success() {
        return Err(format!("codex 비정상 종료: {status} — {tail}"));
    }
    let text = std::fs::read_to_string(&out_file)
        .map_err(|e| format!("codex 결과 파일 없음({e}) — {tail}"))?;
    let _ = std::fs::remove_file(&out_file);
    if text.trim().is_empty() {
        return Err(format!("codex 결과 비어 있음 — {tail}"));
    }
    Ok(text.trim().to_string())
}

fn run_agent_text_once(req: &AgentRequest, env: &[(String, String)]) -> Result<String, String> {
    if provider_kind() == "codex" {
        return run_codex_once(req);
    }
    // claude 발견 = 로그인셸 해석(sh -lc) — GUI(Finder) 실행 앱의 자식은 셸 PATH 를 상속받지 못해
    // PATH 의존 spawn 이 os error 2 로 죽는다(GUI PATH 함정). 사이드카 자신의 자식 발견은 사이드카 책임.
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-lc", r#"exec claude "$@""#, "claude"]);
    for a in claude_args(req) {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()); // stderr 는 상속(claude 경고 그대로 보임, 파이프 deadlock 방지)
                                // 부모 env 격리 후 인증 env 만 주입(누수 방지). PATH·HOME 유지.
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.env("HOME", home);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    // 중첩 claude 가드(공식 nesting guard, claude-code#32618 · agent-sdk#573): 이 사이드카가 Claude Code 세션
    // 안에서 스폰되면 자식 claude 가 부모의 CLAUDECODE/CLAUDE_CODE_* 를 상속해 "cannot launch inside another
    // Claude Code session" 로 hang 한다. 자식 env 에서 그 신호를 자동 제거 → 앱 안이든 밖이든 신선 인스턴스로
    // 실행(env_remove 는 미설정 시 no-op). stdin=null 로 손자 hang(stdin 대기) 방지.
    for k in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_EXECPATH",
    ] {
        cmd.env_remove(k);
    }
    cmd.stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("spawn claude: {e}"))?;
    let stdout = child.stdout.take().ok_or("claude stdout 없음")?;
    // stream-json 소비는 리더 스레드에서 — 메인 스레드는 wait_timeout 으로 하드캡을 건다.
    // (종전 GNU timeout 이 하던 hung 방지: 캡 도달 시 kill → stdout EOF → 리더 종료.)
    let reader = std::thread::spawn(move || {
        let mut run_stream = open_run_stream(); // run catalog — 원시 stream 보존(띵킹 포함, tail -f 모니터링)
        let mut transient_529 = false; // stream 중 [text] API Error 529/overloaded 감지.
                                       // stream-json: 한 줄 = 한 이벤트. 모두 stderr 로 흘려 관측(system·think·tool·subagent·task·…),
                                       // 최종 type=result 의 result 텍스트만 반환값으로 모은다.
        let mut result_text = String::new();
        for line in BufReader::new(stdout).lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            let t = line.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(f) = run_stream.as_mut() {
                use std::io::Write;
                let _ = writeln!(f, "{t}");
            }
            match serde_json::from_str::<Value>(t) {
                Ok(ev) => {
                    print_event(&ev);
                    if event_signals_529(&ev) {
                        transient_529 = true;
                    }
                    if ev.get("type").and_then(|x| x.as_str()) == Some("result") {
                        if let Some(s) = ev.get("result").and_then(|r| r.as_str()) {
                            result_text = s.to_string();
                        }
                    }
                }
                Err(_) => eprintln!("  [stream:raw] {}", t.chars().take(200).collect::<String>()),
            }
        }
        (result_text, transient_529)
    });
    use wait_timeout::ChildExt;
    let status = match child
        .wait_timeout(std::time::Duration::from_secs(req.timeout_secs))
        .map_err(|e| format!("wait claude: {e}"))?
    {
        Some(st) => st,
        None => {
            // 하드캡 도달 — kill 후 reap. 리더는 stdout EOF 로 자연 종료.
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(format!(
                "claude 타임아웃({}s) — 강제 종료(hung 방지 하드캡)",
                req.timeout_secs
            ));
        }
    };
    let (result_text, transient_529) = reader
        .join()
        .map_err(|_| "claude stream 리더 스레드 panic".to_string())?;
    if !status.success() {
        if transient_529 {
            return Err("529 과부하 — claude 비정상 종료(일시적, backoff 대상)".into());
        }
        return Err(format!("claude 비정상 종료: {status}"));
    }
    if result_text.is_empty() {
        return Err("claude 스트림에 type=result 텍스트 없음".into());
    }
    Ok(result_text)
}

/// print_event — stream-json 이벤트 1개를 사람이 읽을 한 줄로 stderr 출력.
/// 모든 stream 타입을 표면화: system·text·think·tool_use(→WebSearch)·tool_result·그 외(subagent/task/agentteam 등).
fn print_event(ev: &Value) {
    let ty = ev.get("type").and_then(|t| t.as_str()).unwrap_or("?");
    match ty {
        "system" => {
            // 제공자에 따라 thinking 토큰마다 system 이벤트가 온다. 도배(매 토큰)는 막되,
            // 전혀 안 찍으면 긴 thinking 동안 멈춘 듯 보이므로 주기적 heartbeat 로 살아있음을 표시.
            let sub = ev.get("subtype").and_then(|s| s.as_str()).unwrap_or("");
            if sub == "init" {
                THINK_BEAT.store(0, Ordering::Relaxed);
                eprintln!("  [system] init");
            } else {
                let n = THINK_BEAT.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 20 == 0 {
                    eprintln!("  [thinking… {n}]");
                }
            }
        }
        // result 시작 시 다음 호출 heartbeat 초기화.
        "result" => {
            THINK_BEAT.store(0, Ordering::Relaxed);
        }
        "assistant" | "user" => {
            if let Some(blocks) = ev
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            {
                for b in blocks {
                    match b.get("type").and_then(|t| t.as_str()).unwrap_or("?") {
                        "text" => eprintln!(
                            "  [text] {}",
                            snip(b.get("text").and_then(|x| x.as_str()).unwrap_or(""))
                        ),
                        "thinking" => eprintln!(
                            "  [think] {}",
                            snip(b.get("thinking").and_then(|x| x.as_str()).unwrap_or(""))
                        ),
                        "tool_use" => eprintln!(
                            "  [tool→] {} {}",
                            b.get("name").and_then(|n| n.as_str()).unwrap_or("?"),
                            snip(&b.get("input").map(|i| i.to_string()).unwrap_or_default())
                        ),
                        "tool_result" => eprintln!("  [tool←] {}", snip(&block_text(b))),
                        o => eprintln!("  [{o}]"),
                    }
                }
            }
        }
        o => eprintln!("  [{o}] {}", snip(&ev.to_string())),
    }
}
fn snip(s: &str) -> String {
    s.replace('\n', " ").chars().take(180).collect()
}
fn block_text(b: &Value) -> String {
    match b.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(" "),
        _ => String::new(),
    }
}

/// run_agent — claude -p 로 agent 실행, result 의 JSON 을 파싱해 반환.
/// env = (key, value) 쌍(인증 프로필). 코어가 주입하는 형태를 그대로 받는다.
pub fn run_agent(req: &AgentRequest, env: &[(String, String)]) -> Result<Value, String> {
    let text = run_agent_text(req, env)?;
    parse_json_lenient(&text)
}

/// parse_json_lenient — 코드펜스(```json) 제거 후 JSON 파싱. 앞뒤 prose 가 있으면
/// 첫 `{`~마지막 `}` 구간 추출 시도(모델이 펜스/설명을 붙이는 경우 대비).
pub fn parse_json_lenient(text: &str) -> Result<Value, String> {
    let t = text.trim();
    let stripped = strip_code_fence(t);
    if let Ok(v) = serde_json::from_str::<Value>(stripped.trim()) {
        return Ok(v);
    }
    // 첫 { ~ 매칭 } 추출(브레이스 균형).
    if let Some(slice) = extract_balanced_object(stripped) {
        if let Ok(v) = serde_json::from_str::<Value>(&slice) {
            return Ok(v);
        }
    }
    Err(format!(
        "agent 출력 JSON 파싱 실패. head={}",
        stripped.chars().take(200).collect::<String>()
    ))
}

fn strip_code_fence(t: &str) -> &str {
    let t = t.trim();
    if let Some(rest) = t.strip_prefix("```json").or_else(|| t.strip_prefix("```")) {
        let rest = rest.trim_start_matches('\n');
        if let Some(end) = rest.rfind("```") {
            return &rest[..end];
        }
        return rest;
    }
    t
}

fn extract_balanced_object(t: &str) -> Option<String> {
    let bytes = t.as_bytes();
    let start = t.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for i in start..bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(t[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests_529 {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_529_in_assistant_content_block() {
        // 실측 형태: assistant 이벤트의 content[] text 블록으로 과부하 안내가 온다.
        let ev = json!({ "type": "assistant", "message": { "content": [
            { "type": "text", "text": "API Error: 529 [1305][The service may be temporarily overloaded, please try again later]" }
        ] } });
        assert!(event_signals_529(&ev));
        let ev2 = json!({ "type": "assistant", "message": { "content": [ { "type": "text", "text": "정상 응답" } ] } });
        assert!(!event_signals_529(&ev2));
    }

    #[test]
    fn detects_529_in_top_level_text_and_ignores_others() {
        assert!(event_signals_529(
            &json!({ "type": "text", "text": "overloaded" })
        ));
        // result 는 산출 채널 — 본문에 "529" 가 있어도 과부하 신호가 아니다. 구조 필드만 신호.
        assert!(!event_signals_529(
            &json!({ "type": "result", "result": "요건 529 관련 서술" })
        ));
        assert!(event_signals_529(
            &json!({ "type": "result", "is_error": true, "api_error_status": 529, "result": "API Error: 529" })
        ));
        assert!(!event_signals_529(
            &json!({ "type": "result", "api_error_status": 200 })
        ));
        assert!(!event_signals_529(
            &json!({ "type": "system", "subtype": "init" })
        ));
    }
}

#[cfg(test)]
mod tests_codex_schema {
    use super::*;

    #[test]
    fn openai_normalize_makes_strict_and_nullable() {
        let mut v: Value = serde_json::json!({
            "type": "object", "required": ["a"],
            "properties": {
                "a": {"type": "string"},
                "b": {"type": "number"},
                "c": {"type": "object", "properties": {"d": {"type": "string"}}}
            }
        });
        normalize_schema_for_openai(&mut v);
        assert_eq!(v["additionalProperties"], serde_json::json!(false));
        let req: Vec<&str> = v["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap())
            .collect();
        assert!(
            req.contains(&"a") && req.contains(&"b") && req.contains(&"c"),
            "전 키 required"
        );
        assert_eq!(
            v["properties"]["b"]["type"],
            serde_json::json!(["number", "null"]),
            "선택 키는 nullable"
        );
        assert_eq!(
            v["properties"]["a"]["type"],
            serde_json::json!("string"),
            "원래 필수는 그대로"
        );
        assert_eq!(
            v["properties"]["c"]["additionalProperties"],
            serde_json::json!(false),
            "중첩 object 도"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_catalog_uses_a_regular_pointer_to_an_immutable_stream() {
        use std::io::Write;
        let root = std::fs::canonicalize(std::env::temp_dir())
            .unwrap()
            .join(format!(
                "soksak-workflow-run-catalog-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir(&root).unwrap();
        let mut stream = create_run_stream(&root).expect("create run stream");
        writeln!(stream, "{{\"type\":\"result\"}}").unwrap();
        drop(stream);

        let pointer_path = root.join("latest.json");
        assert!(!std::fs::symlink_metadata(&pointer_path)
            .unwrap()
            .file_type()
            .is_symlink());
        let pointer: Value =
            serde_json::from_slice(&std::fs::read(&pointer_path).unwrap()).unwrap();
        let relative = pointer.get("stream").and_then(Value::as_str).unwrap();
        assert!(!relative.contains('/') && relative.ends_with(".jsonl"));
        assert_eq!(
            std::fs::read_to_string(root.join(relative)).unwrap(),
            "{\"type\":\"result\"}\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parse_plain_json() {
        assert_eq!(parse_json_lenient(r#"{"a":1}"#).unwrap(), json!({"a":1}));
    }

    #[test]
    fn parse_code_fenced() {
        assert_eq!(
            parse_json_lenient("```json\n{\"a\":1}\n```").unwrap(),
            json!({"a":1})
        );
        assert_eq!(
            parse_json_lenient("```\n{\"a\":1}\n```").unwrap(),
            json!({"a":1})
        );
    }

    #[test]
    fn parse_with_surrounding_prose() {
        let v =
            parse_json_lenient("Here is the result:\n{\"angles\":[\"x\",\"y\"]}\nDone.").unwrap();
        assert_eq!(v["angles"], json!(["x", "y"]));
    }

    /// system_prompt 가 Some 면 --append-system-prompt <내용> 이 args 에 추가된다(user prompt 와 분리).
    #[test]
    fn system_prompt_appends_flag_when_some() {
        let req = AgentRequest {
            prompt: "USER_PROMPT".into(),
            model: "haiku",
            text_only: false,
            allowed_tools: vec![],
            timeout_secs: 10,
            system_prompt: Some("SKILL_AST_SYSTEM".into()),
            schema: None,
            effort: "xhigh".into(),
        };
        let args = claude_args(&req);
        assert!(
            args.contains(&"-p".into()) && args.contains(&"USER_PROMPT".into()),
            "user prompt(-p) 유지"
        );
        let i = args
            .iter()
            .position(|a| a == "--append-system-prompt")
            .expect("system flag 누락");
        assert_eq!(
            args[i + 1],
            "SKILL_AST_SYSTEM",
            "system_prompt 내용이 flag 바로 뒤"
        );
    }

    /// system_prompt None 이면 --append-system-prompt 가 아예 안 붙는다(종래 동작).
    #[test]
    fn system_prompt_omitted_when_none() {
        let req = AgentRequest {
            prompt: "p".into(),
            model: "haiku",
            text_only: false,
            allowed_tools: vec![],
            timeout_secs: 10,
            system_prompt: None,
            schema: None,
            effort: "xhigh".into(),
        };
        let args = claude_args(&req);
        assert!(
            !args.iter().any(|a| a == "--append-system-prompt"),
            "None 이면 system flag 미부착"
        );
    }

    fn req(text_only: bool) -> AgentRequest<'static> {
        AgentRequest {
            prompt: "p".into(),
            model: "glm-5.2",
            text_only,
            allowed_tools: vec![],
            timeout_secs: 10,
            system_prompt: None,
            schema: Some(json!({ "type": "object" })),
            effort: "max".into(),
        }
    }

    /// is_zai_url — ANTHROPIC_BASE_URL 로 z.ai/glm 프로필 판별.
    #[test]
    fn is_zai_url_detects_zai_endpoint() {
        assert!(
            is_zai_url(Some("https://api.z.ai/api/anthropic")),
            "z.ai 엔드포인트"
        );
        assert!(
            is_zai_url(Some("https://open.bigmodel.cn/...")),
            "zhipu bigmodel"
        );
        assert!(
            !is_zai_url(Some("https://api.anthropic.com")),
            "real Anthropic"
        );
        assert!(!is_zai_url(None), "미설정 = real claude");
    }

    /// 규칙(회귀): 프로브 검증된 서버만 배선. z.ai 프로필 → context7 + z.ai web_search_prime(native WebSearch
    /// 미지원 대체). real claude → context7 만(native WebSearch 사용). 두 경우 다 ddg/비전 배선 금지. 키 옵트인.
    #[test]
    fn default_search_servers_branches_by_profile() {
        // z.ai/glm 프로필 → context7 + z.ai 정식 웹검색.
        let zai = default_search_servers(true, Some("TOK"), None, None);
        let s = zai["mcpServers"].as_object().unwrap();
        assert!(s.contains_key("context7"), "context7(docs/버전) 배선");
        assert!(s.contains_key("web-search-prime"), "z.ai 정식 웹검색 배선");
        assert_eq!(
            zai["mcpServers"]["web-search-prime"]["url"],
            "https://api.z.ai/api/mcp/web_search_prime/mcp"
        );
        assert_eq!(
            zai["mcpServers"]["web-search-prime"]["headers"]["Authorization"],
            "Bearer TOK"
        );
        // real claude 프로필 → context7 만(웹검색은 native WebSearch). 웹검색 MCP·비전·ddg 없음.
        let cl = default_search_servers(false, Some("TOK"), None, None);
        let s2 = cl["mcpServers"].as_object().unwrap();
        assert!(
            s2.contains_key("context7"),
            "real claude 도 context7(docs)은 유용"
        );
        assert!(
            !s2.contains_key("web-search-prime"),
            "real claude 는 native WebSearch — z.ai 웹검색 미배선"
        );
        assert!(
            !cl.to_string().contains("ddg") && !cl.to_string().contains("@z_ai/mcp-server"),
            "ddg·비전 배선 금지"
        );
        // 키 프리미엄 옵트인(두 프로필 공용).
        let prem = default_search_servers(false, None, Some("TAV"), Some("BR"));
        let s3 = prem["mcpServers"].as_object().unwrap();
        assert!(
            s3.contains_key("tavily") && s3.contains_key("brave"),
            "키 있으면 프리미엄 추가"
        );
    }

    /// z.ai/glm 발굴 에이전트: context7 배선·grant + WebSearch(미지원)·Bash 차단, native WebSearch grant 안 함.
    /// (z.ai web_search_prime 배선 자체는 토큰 의존이라 default_search_servers 순수 테스트가 커버.)
    #[test]
    fn claude_args_zai_blocks_websearch_wires_zai_mcp() {
        let args = claude_args_impl(&req(false), true);
        let mi = args
            .iter()
            .position(|a| a == "--mcp-config")
            .expect("--mcp-config");
        assert!(
            args[mi + 1].contains("@upstash/context7-mcp"),
            "context7 배선: {}",
            args[mi + 1]
        );
        let ai = args
            .iter()
            .position(|a| a == "--allowedTools")
            .expect("allowedTools");
        assert!(
            args[ai + 1].contains("mcp__context7"),
            "MCP grant: {}",
            args[ai + 1]
        );
        assert!(
            !di_has_websearch(&args[ai + 1]),
            "glm 은 native WebSearch grant 안 함: {}",
            args[ai + 1]
        );
        let di = args
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("disallowedTools");
        assert!(
            args[di + 1].contains("WebSearch") && args[di + 1].contains("Bash"),
            "WebSearch·Bash 차단"
        );
    }

    /// real claude 발굴 에이전트: native WebSearch 허용(grant, 차단 안 함) + context7 배선. z.ai 웹검색 없음.
    #[test]
    fn claude_args_real_claude_allows_native_websearch() {
        let args = claude_args_impl(&req(false), false);
        let mi = args
            .iter()
            .position(|a| a == "--mcp-config")
            .expect("--mcp-config");
        assert!(
            args[mi + 1].contains("@upstash/context7-mcp"),
            "context7 배선"
        );
        assert!(
            !args[mi + 1].contains("web_search_prime"),
            "real claude 엔 z.ai 웹검색 미배선"
        );
        let ai = args
            .iter()
            .position(|a| a == "--allowedTools")
            .expect("allowedTools");
        assert!(
            args[ai + 1].contains("WebSearch") && args[ai + 1].contains("mcp__context7"),
            "native WebSearch + context7 grant: {}",
            args[ai + 1]
        );
        let di = args
            .iter()
            .position(|a| a == "--disallowedTools")
            .expect("disallowedTools");
        assert!(
            !di_has_websearch(&args[di + 1]) && args[di + 1].contains("Bash"),
            "WebSearch 차단 안 함, Bash 는 차단: {}",
            args[di + 1]
        );
    }

    fn di_has_websearch(disallowed: &str) -> bool {
        disallowed.split_whitespace().any(|t| t == "WebSearch")
    }

    /// text_only(저작) 에이전트엔 검색 MCP 를 배선하지 않는다(순수 텍스트 반환), 프로필 무관.
    #[test]
    fn search_mcp_absent_for_text_only() {
        for zai in [true, false] {
            let args = claude_args_impl(&req(true), zai);
            assert!(
                !args.iter().any(|a| a == "--mcp-config"),
                "저작 에이전트엔 검색 MCP 미배선(zai={zai})"
            );
        }
    }
}

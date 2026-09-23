pub mod github;

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, OnceLock},
    time::Instant,
};

use actix_web::{
    App, HttpRequest, HttpResponse, HttpServer, Responder, http::header, middleware::DefaultHeaders,
    web,
};

use crate::github::get_python_code;

const PAGE_HTML: &str = include_str!("ui/page.html");
const EXPLAINER_HTML: &str = include_str!("ui/explainer.html");

static CONNECTIONS: OnceLock<Mutex<Vec<(String, Instant)>>> = OnceLock::new();
static RATE_LIMITED: OnceLock<Mutex<Vec<(String, Instant)>>> = OnceLock::new();
static RATE_LIMITED_THIS_INSTANCE: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static PEER_CONNECTIONS: OnceLock<Mutex<Vec<(String, Instant)>>> = OnceLock::new();
static PEER_RATE_LIMITED: OnceLock<Mutex<Vec<(String, Instant)>>> = OnceLock::new();

const MAX_PER_TEN_SECONDS: u32 = 70; // ten full page loads
const MAX_PER_TEN_SECONDS_PEER: u32 = 700; // one hundred full page loads
const RATE_LIMIT_MINUTES_FIRST: u64 = 1;
const RATE_LIMIT_MINUTES_SECOND: u64 = 30;
const RATE_LIMIT_MINUTES_PEER: u64 = 5;

fn python_string_literal(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    out.push('"');
    for c in input.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// exit()/quit() at the end of a script raises SystemExit, which the runtime
// otherwise reports as an uncaught traceback even though it's a normal exit.
fn wrap_python_code(code: &str) -> String {
    // MicroPython doesn't normalize CRLF/CR line endings the way CPython does,
    // so files with Windows-style newlines render garbled in the terminal.
    let code = code.replace("\r\n", "\n").replace('\r', "\n");
    format!(
        "try:\n    exec(compile({}, \"script.py\", \"exec\"))\nexcept SystemExit:\n    pass\n",
        python_string_literal(&code)
    )
}

fn escape_html(input: &str) -> String {
    input
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace("\"", "&quot;")
        .replace("'", "&#39;")
}

fn query_param(req: &HttpRequest, key: &str) -> Option<String> {
    web::Query::<HashMap<String, String>>::from_query(req.query_string())
        .ok()?
        .into_inner()
        .remove(key)
        .filter(|x| !x.is_empty())
}

fn extract_global<'a, T>(input: &'a OnceLock<Mutex<Vec<T>>>) -> MutexGuard<'a, Vec<T>> {
    input
        .get_or_init(|| Mutex::new(vec![]))
        .lock()
        .unwrap_or_else(|x| x.into_inner())
}

async fn dispatch(req: HttpRequest) -> impl Responder {
    {
        let ip = req
            .connection_info()
            .realip_remote_addr()
            .map(|x| x.to_string())
            .unwrap_or(String::new());

        let peer_ip = req
            .peer_addr()
            .map(|x| x.ip().to_string())
            .unwrap_or(String::new());
        if ip != peer_ip {
            eprintln!("forwarded ip {ip} claimed by peer {peer_ip}");
        }

        let mut peer_rate_limits = extract_global(&PEER_RATE_LIMITED);

        *peer_rate_limits = peer_rate_limits
            .iter()
            .filter(|x| {
                std::time::Instant::now().duration_since(x.1).as_secs()
                    < RATE_LIMIT_MINUTES_PEER * 60
            })
            .map(|x| (x.0.clone(), x.1))
            .collect::<Vec<_>>();

        if peer_rate_limits.iter().any(|x| x.0 == peer_ip) {
            return HttpResponse::TooManyRequests()
                .content_type("text/plain; charset=utf-8")
                .body("rate limited, please wait");
        }

        let mut peer_connections = extract_global(&PEER_CONNECTIONS);
        *peer_connections = peer_connections
            .iter()
            .filter(|x| std::time::Instant::now().duration_since(x.1).as_secs() < 10)
            .map(|x| (x.0.clone(), x.1))
            .collect::<Vec<_>>();

        let peer_count = peer_connections.iter().filter(|x| x.0 == peer_ip).count() as u32;
        if peer_count >= MAX_PER_TEN_SECONDS_PEER {
            peer_rate_limits.push((peer_ip, std::time::Instant::now()));
            return HttpResponse::TooManyRequests()
                .content_type("text/plain; charset=utf-8")
                .body("rate limited, please wait");
        }

        peer_connections.push((peer_ip, std::time::Instant::now()));

        let mut rate_limited = extract_global(&RATE_LIMITED);

        let mut connections = extract_global(&CONNECTIONS);

        let mut rate_limited_this_instance = extract_global(&RATE_LIMITED_THIS_INSTANCE);
        rate_limited_this_instance.sort();
        rate_limited_this_instance.dedup();

        fn check_rate_limit_not_ready_to_clear(
            x: &(String, Instant),
            rate_limited_this_instance: &[String],
        ) -> bool {
            let rate_limit_time = if !rate_limited_this_instance.contains(&x.0) {
                RATE_LIMIT_MINUTES_FIRST * 60
            } else {
                RATE_LIMIT_MINUTES_SECOND * 60
            };

            std::time::Instant::now().duration_since(x.1).as_secs() < rate_limit_time
        }

        let rate_limited_this_instance_immutable = rate_limited_this_instance.clone();

        let expired = rate_limited
            .iter()
            .filter(|x| {
                !check_rate_limit_not_ready_to_clear(x, &rate_limited_this_instance_immutable)
            })
            .map(|x| x.0.clone())
            .collect::<Vec<_>>();

        *rate_limited = rate_limited
            .iter()
            .filter(|x| {
                check_rate_limit_not_ready_to_clear(x, &rate_limited_this_instance_immutable)
            })
            .map(|x| (x.0.clone(), x.1))
            .collect::<Vec<_>>();

        rate_limited_this_instance.extend(expired);

        if rate_limited.iter().any(|x| x.0 == ip) {
            return HttpResponse::TooManyRequests()
                .content_type("text/plain; charset=utf-8")
                .body("rate limited, please wait");
        }

        *connections = connections
            .iter()
            .filter(|x| std::time::Instant::now().duration_since(x.1).as_secs() < 10)
            .map(|x| (x.0.clone(), x.1))
            .collect::<Vec<_>>();
        connections.sort();
        let mut count = 1;
        let mut last = String::new();
        for (ip, _) in connections.iter() {
            if *ip == last {
                count += 1;
            } else {
                count = 1;
            }
            if count >= MAX_PER_TEN_SECONDS {
                rate_limited.push((ip.clone(), std::time::Instant::now()));
            }

            last = ip.clone();
        }

        connections.push((ip, std::time::Instant::now()));
    }

    let path = req.path();
    let terms = path
        .split("/")
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();

    if terms.is_empty() {
        return HttpResponse::Ok()
            .content_type("text/html; charset=utf-8")
            .body(EXPLAINER_HTML);
    }

    if terms.len() < 2 {
        eprintln!("Invalid path: {}", path);
        return HttpResponse::Ok()
            .content_type("text/plain; charset=utf-8")
            .body("Invalid path");
    }

    let user = terms[0].to_string();
    let repo = terms[1].to_string();
    let file = query_param(&req, "file");
    if terms.len() != 0 {
        match *terms.last().unwrap() {
            "term_style.css" => {
                return HttpResponse::Ok()
                    .content_type("text/css; charset=utf-8")
                    .body(include_str!("ui/term_style.css"));
            }
            "term_config.js" => {
                return HttpResponse::Ok()
                    .content_type("text/javascript; charset=utf-8")
                    .body(include_str!("ui/term_config.js"));
            }
            "script.py" if user == "__test" => {
                let python_code = "print(\"line one\")\nprint(\"line two\")\nfor i in range(5):\n    print(i)\nexit()\n".to_string();
                return HttpResponse::Ok()
                    .content_type("text/plain; charset=utf-8")
                    .body(wrap_python_code(&python_code));
            }
            "script.py" => {
                let python_code = match get_python_code(&user, &repo, file.as_deref()).await {
                    Ok(code) => code,
                    Err(e) => {
                        eprintln!("Error: {}", e);
                        return HttpResponse::InternalServerError()
                            .content_type("text/plain; charset=utf-8")
                            .body("Internal server error");
                    }
                };
                return HttpResponse::Ok()
                    .content_type("text/plain; charset=utf-8")
                    .body(wrap_python_code(&python_code));
            }
            _ => {}
        }
    }

    let query = match req.query_string() {
        "" => String::new(),
        q => format!("?{}", escape_html(q)),
    };

    // PyScript only fetches a worker's config URL if it ends in .json/.toml,
    // so a query string can't be used; the config is inlined instead.
    let packages = query_param(&req, "packages")
        .map(|x| {
            x.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let config = serde_json::json!({ "packages": packages }).to_string();

    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(
            PAGE_HTML
                .replace("{PAGE_TITLE}", &escape_html(&repo))
                .replace("{USER}", &escape_html(&user))
                .replace("{REPO}", &escape_html(&repo))
                .replace("{RUNTIME}", "py")
                .replace("{CONFIG}", &escape_html(&config))
                .replace("{QUERY}", &query),
        )
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    HttpServer::new(|| {
        App::new()
            .wrap(
                DefaultHeaders::new()
                    .add((header::CROSS_ORIGIN_OPENER_POLICY, "same-origin"))
                    .add((header::CROSS_ORIGIN_EMBEDDER_POLICY, "require-corp"))
                    .add((header::CROSS_ORIGIN_RESOURCE_POLICY, "cross-origin"))
                    .add((header::X_CONTENT_TYPE_OPTIONS, "nosniff")),
            )
            .default_service(actix_web::web::to(dispatch))
    })
    .bind(("0.0.0.0", 8080))?
    .run()
    .await
}

pub mod github;

use actix_web::{
    App, HttpRequest, HttpResponse, HttpServer, Responder,
    http::header,
    middleware::DefaultHeaders,
};

use crate::github::get_python_code;

const PAGE_HTML: &str = include_str!("ui/page.html");

async fn dispatch(req: HttpRequest) -> impl Responder {
    let path = req.path();
    let terms = path
        .split("/")
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();

    if terms.len() < 2 {
        return HttpResponse::Ok().body("Invalid path");
    }

    let user = terms[0].to_string();
    let repo = terms[1].to_string();
    if terms.len() != 0 {
        match *terms.last().unwrap() {
            "term_style.css" => return HttpResponse::Ok().body(include_str!("ui/term_style.css")),
            "conf.json" => return HttpResponse::Ok().body("{}"),
            "script.py" => {
                let python_code = match get_python_code(&user, &repo).await {
                    Ok(code) => code,
                    Err(e) => return HttpResponse::Ok().body(format!("Error: {}", e)),
                };
                return HttpResponse::Ok().body(python_code);
            }
            _ => {}
        }
    }

    HttpResponse::Ok().body(
        PAGE_HTML
            .replace("{PAGE_TITLE}", &repo.to_string())
            .replace("{USER}", &user)
            .replace("{REPO}", &repo),
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
                    .add((header::CROSS_ORIGIN_RESOURCE_POLICY, "cross-origin")),
            )
            .default_service(actix_web::web::to(dispatch))
    })
    .bind(("127.0.0.1", 8080))?
    .run()
    .await
}

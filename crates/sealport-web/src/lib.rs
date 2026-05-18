use axum::{
    Router,
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::get,
};
use leptos::prelude::*;

const SITE_CSS: &str = include_str!("site.css");

pub fn app() -> Router {
    Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/assets/site.css", get(stylesheet))
}

async fn home() -> Html<String> {
    Html(render_homepage())
}

async fn healthz() -> &'static str {
    "ok\n"
}

async fn stylesheet() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        SITE_CSS,
    )
}

pub fn render_homepage() -> String {
    view! { <Homepage/> }.to_html()
}

#[component]
fn Homepage() -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta
                    name="description"
                    content="SealPort is a planned all-Rust encrypted backup CLI for self-hosted, scriptable backups."
                />
                <title>"SealPort - encrypted backups, same everywhere"</title>
                <link rel="stylesheet" href="/assets/site.css"/>
            </head>
            <body>
                <a class="skip-link" href="#main">"Skip to content"</a>
                <header class="site-header">
                    <a class="brand" href="/" aria-label="SealPort home">
                        <span class="brand-mark" aria-hidden="true">"S"</span>
                        <span>"SealPort"</span>
                    </a>
                    <nav aria-label="Primary navigation">
                        <a href="#status">"Status"</a>
                        <a href="#operators">"Operators"</a>
                        <a href="#roadmap">"Roadmap"</a>
                        <a href="https://github.com/dunamismax/sealport">"GitHub"</a>
                    </nav>
                </header>
                <main id="main">
                    <section class="hero" aria-labelledby="hero-title">
                        <div class="hero-art" aria-hidden="true">
                            <div class="vault-ring ring-one"></div>
                            <div class="vault-ring ring-two"></div>
                            <div class="signal-grid">
                                <span></span>
                                <span></span>
                                <span></span>
                                <span></span>
                                <span></span>
                                <span></span>
                            </div>
                            <div class="repo-card">
                                <span>"repo/v0"</span>
                                <strong>"authenticated objects"</strong>
                            </div>
                            <div class="restore-card">
                                <span>"restore drill"</span>
                                <strong>"latest -> verified target"</strong>
                            </div>
                        </div>
                        <div class="hero-shell">
                            <p class="eyebrow">"sealport.cc"</p>
                            <h1 id="hero-title">"Encrypted backup CLI."</h1>
                            <p class="lead">
                                "SealPort is an all-Rust backup command being built for owned, inspectable, scriptable backups with client-side encryption and boring restore paths."
                            </p>
                            <div class="hero-actions" aria-label="Primary links">
                                <a class="button primary" href="https://github.com/dunamismax/sealport">"View source"</a>
                                <a class="button secondary" href="#status">"Read project status"</a>
                            </div>
                            <dl class="hero-facts" aria-label="Product facts">
                                <div>
                                    <dt>"Command"</dt>
                                    <dd>"sealport"</dd>
                                </div>
                                <div>
                                    <dt>"Mode"</dt>
                                    <dd>"CLI only"</dd>
                                </div>
                                <div>
                                    <dt>"Backends"</dt>
                                    <dd>"Local + S3 target"</dd>
                                </div>
                            </dl>
                        </div>
                    </section>

                    <div class="command-band intro-command" aria-label="SealPort command preview">
                        <div class="command-head">
                            <span>"planned operator flow"</span>
                            <strong>"stdout stays machine-readable"</strong>
                        </div>
                        <pre><code>"$ sealport init s3://company-backups/laptops\n$ sealport backup ~/Documents --tag laptop --jsonl\n{\"event\":\"backup.started\",\"repository\":\"redacted\"}\n{\"event\":\"backup.finished\",\"snapshot\":\"planned\"}\n$ sealport restore latest ~/restore-test"</code></pre>
                    </div>

                    <section class="status-panel" id="status" aria-label="Current project status">
                        <div class="status-copy">
                            <p class="eyebrow">"Current status"</p>
                            <h2>"Pre-v1 foundation, honest by default."</h2>
                            <p>
                                "The Rust workspace, CLI shell, config/output contracts, initial crypto primitives, storage abstractions, and this public homepage exist. The backup and restore engine is still under construction."
                            </p>
                        </div>
                        <div class="status-matrix">
                            <article>
                                <span class="label">"Implemented"</span>
                                <strong>"CLI foundation"</strong>
                                <p>"version, completions, config precedence, JSON, JSONL, golden tests"</p>
                            </article>
                            <article>
                                <span class="label">"Implemented"</span>
                                <strong>"Crypto groundwork"</strong>
                                <p>"master keys, passphrase unlock, subkeys, authenticated envelopes"</p>
                            </article>
                            <article>
                                <span class="label">"Implemented"</span>
                                <strong>"Storage groundwork"</strong>
                                <p>"local backend, S3-compatible backend, fake store, capability model"</p>
                            </article>
                            <article class="pending">
                                <span class="label">"Not done yet"</span>
                                <strong>"Backup engine"</strong>
                                <p>"snapshot creation, restore pipeline, check, forget, prune, v1 release artifacts"</p>
                            </article>
                        </div>
                    </section>

                    <section class="section command-contract" aria-labelledby="contract-title">
                        <div class="section-heading">
                            <p class="eyebrow">"Command contract"</p>
                            <h2 id="contract-title">"Built for shells, logs, runbooks, and restore drills."</h2>
                        </div>
                        <div class="contract-grid">
                            <article>
                                <span>"01"</span>
                                <h3>"Stdout is data"</h3>
                                <p>"Human progress and diagnostics stay on stderr so scripts can trust stdout."</p>
                            </article>
                            <article>
                                <span>"02"</span>
                                <h3>"JSON and JSONL surfaces"</h3>
                                <p>"Single-document output for state, event streams for long operations."</p>
                            </article>
                            <article>
                                <span>"03"</span>
                                <h3>"Dry-run destructive work"</h3>
                                <p>"Forget, prune, and other destructive commands are planned around explicit dry runs."</p>
                            </article>
                            <article>
                                <span>"04"</span>
                                <h3>"Exit codes are part of the API"</h3>
                                <p>"Failure families are documented before v1 and treated as automation contracts."</p>
                            </article>
                        </div>
                    </section>

                    <section class="section operator-section" id="operators" aria-labelledby="operators-title">
                        <div class="section-heading compact">
                            <p class="eyebrow">"Operator priorities"</p>
                            <h2 id="operators-title">"The design is narrow on purpose."</h2>
                            <p>
                                "SealPort is not trying to become a dashboard, daemon, scheduler, server, mount layer, or compatibility shim. The product center is encrypted backup and reliable restore from a local or S3-compatible repository."
                            </p>
                        </div>
                        <div class="operator-grid">
                            <article>
                                <h3>"Restore first"</h3>
                                <p>"Backup features are judged by whether they make restore safer, clearer, and easier to verify."</p>
                            </article>
                            <article>
                                <h3>"Client-side encryption"</h3>
                                <p>"Contents, names, directory shape, manifests, indexes, and sensitive config are designed to stay encrypted."</p>
                            </article>
                            <article>
                                <h3>"Evidence before claims"</h3>
                                <p>"Platform support waits for CI, tests, release artifacts, and observed behavior."</p>
                            </article>
                            <article>
                                <h3>"Original repository format"</h3>
                                <p>"SealPort does not read or write restic, rustic, Borg, Kopia, or rclone-native repositories."</p>
                            </article>
                            <article>
                                <h3>"Object-store discipline"</h3>
                                <p>"Immutable objects, retry-safe writes, explicit commit markers, and no required rename assumptions."</p>
                            </article>
                            <article>
                                <h3>"Inspectability"</h3>
                                <p>"Typed docs, structured events, and small crate boundaries keep behavior auditable as the system grows."</p>
                            </article>
                        </div>
                    </section>

                    <section class="section roadmap" id="roadmap" aria-labelledby="roadmap-title">
                        <div class="roadmap-copy">
                            <p class="eyebrow">"Design boundaries"</p>
                            <h2 id="roadmap-title">"The next work is the backup pipeline, then restore."</h2>
                            <p>
                                "The current foundation is useful only if it leads to boring recovery. The public roadmap keeps that pressure visible."
                            </p>
                        </div>
                        <ol class="timeline">
                            <li>
                                <span>"Now"</span>
                                <strong>"Phase 5 - backup pipeline"</strong>
                                <p>"source walking, exclusions, metadata capture, chunking, compression, encryption, manifests"</p>
                            </li>
                            <li>
                                <span>"Next"</span>
                                <strong>"Phase 6 - restore pipeline"</strong>
                                <p>"snapshot selection, path-scoped restore, destination safety, metadata restore, verification"</p>
                            </li>
                            <li>
                                <span>"Then"</span>
                                <strong>"Phase 7+ - listing, check, retention, release"</strong>
                                <p>"snapshots, ls, find, diff, integrity checks, forget, prune, signing, SBOMs"</p>
                            </li>
                        </ol>
                    </section>

                    <section class="section release" aria-labelledby="release-title">
                        <div>
                            <p class="eyebrow">"Self-hosting shape"</p>
                            <h2 id="release-title">"The website is separate marketing infrastructure."</h2>
                            <p>
                                "This site is served by an Axum binary rendering Leptos views on the server. It has no database, no client-side app bundle, and a simple health endpoint for reverse proxies."
                            </p>
                        </div>
                        <div class="deploy">
                            <span>"Binary"</span>
                            <code>"sealport-web"</code>
                            <span>"Default bind"</span>
                            <code>"0.0.0.0:8080"</code>
                            <span>"Override"</span>
                            <code>"SEALPORT_WEB_ADDR=127.0.0.1:8080"</code>
                        </div>
                    </section>
                </main>
                <footer>
                    <span>"SealPort"</span>
                    <a href="https://github.com/dunamismax/sealport">"GitHub"</a>
                    <a href="https://sealport.cc/">"sealport.cc"</a>
                </footer>
            </body>
        </html>
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body, http::Request};
    use tower::ServiceExt;

    #[test]
    fn homepage_renders_honest_project_status() {
        let html = render_homepage();

        assert!(html.contains("Encrypted backup CLI."));
        assert!(html.contains("Pre-v1 foundation"));
        assert!(html.contains("The backup and restore engine is still under construction."));
        assert!(html.contains("sealport-web"));
        assert!(html.contains("/assets/site.css"));
    }

    #[tokio::test]
    async fn routes_serve_homepage_health_and_css() {
        let app = app();

        let home = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(home.status(), StatusCode::OK);

        let health = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::OK);

        let css = app
            .oneshot(
                Request::builder()
                    .uri("/assets/site.css")
                    .body(body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(css.status(), StatusCode::OK);
        assert_eq!(
            css.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/css; charset=utf-8"
        );
    }
}

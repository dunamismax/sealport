use axum::{
    Router,
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::get,
};
use leptos::prelude::*;

const SITE_CSS: &str = include_str!("site.css");
const THEME_JS: &str = include_str!("theme.js");

// Pre-rendered, syntax-highlighted terminal HTML. Kept as a raw string and
// injected via `inner_html` so the Leptos `view!` macro never has to parse
// embedded JSON literals or interleaved spans.
const TERMINAL_HTML: &str = r#"<span class="prompt">$ </span><span class="cmd">ferry</span> <span class="arg">init</span> <span class="arg">s3://company-backups/laptops</span>
<span class="out">repository initialized &middot; format v0 &middot; encrypted</span>

<span class="prompt">$ </span><span class="cmd">ferry</span> <span class="arg">backup</span> <span class="arg">~/Documents</span> <span class="flag">--tag</span> <span class="arg">laptop</span> <span class="flag">--jsonl</span>
<span class="json-key">{</span><span class="json-str">&quot;event&quot;:&quot;backup.started&quot;</span><span class="json-key">,...}</span>
<span class="json-key">{</span><span class="json-str">&quot;event&quot;:&quot;backup.finished&quot;</span><span class="json-key">,...}</span>
<span class="comment"># stdout stays JSONL &middot; progress on stderr</span>

<span class="prompt">$ </span><span class="cmd">ferry</span> <span class="arg">restore</span> <span class="arg">latest</span> <span class="arg">~/restore-test</span>
<span class="out">restored &middot; 14 files &middot; 0 metadata warnings</span>"#;

// Inline early-execution script. Reads the stored or system theme and applies
// it to the documentElement before paint, so the first frame is never the
// wrong theme.
const THEME_INIT_JS: &str = r#"(function(){try{var s=localStorage.getItem('fileferry-theme');var t=s||(window.matchMedia&&window.matchMedia('(prefers-color-scheme: light)').matches?'light':'dark');document.documentElement.setAttribute('data-theme',t);}catch(e){document.documentElement.setAttribute('data-theme','dark');}})();"#;

pub fn app() -> Router {
    Router::new()
        .route("/", get(home))
        .route("/healthz", get(healthz))
        .route("/assets/site.css", get(stylesheet))
        .route("/assets/theme.js", get(theme_script))
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

async fn theme_script() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        THEME_JS,
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
                <meta name="color-scheme" content="dark light"/>
                <meta name="theme-color" content="#0d1117" media="(prefers-color-scheme: dark)"/>
                <meta name="theme-color" content="#ffffff" media="(prefers-color-scheme: light)"/>
                <meta
                    name="description"
                    content="FileFerry is a planned all-Rust encrypted backup CLI for self-hosted, scriptable backups."
                />
                <title>"FileFerry — encrypted backups, same everywhere"</title>
                <link rel="stylesheet" href="/assets/site.css"/>
                <script inner_html=THEME_INIT_JS></script>
            </head>
            <body>
                <a class="skip-link" href="#main">"Skip to content"</a>

                <header class="site-header">
                    <a class="brand" href="/" aria-label="FileFerry home">
                        <span class="brand-mark" aria-hidden="true">"⏵"</span>
                        <span>"FileFerry"</span>
                    </a>
                    <nav class="site-nav" aria-label="Primary navigation">
                        <a href="#status">"Status"</a>
                        <a href="#contract">"Contract"</a>
                        <a href="#security">"Security"</a>
                        <a href="#roadmap">"Roadmap"</a>
                    </nav>
                    <div class="header-spacer"></div>
                    <div class="header-actions">
                        <a
                            class="icon-link"
                            href="https://github.com/dunamismax/fileferry"
                            aria-label="View FileFerry on GitHub"
                            title="GitHub"
                        >
                            <svg viewBox="0 0 16 16" aria-hidden="true">
                                <path d="M8 0C3.58 0 0 3.58 0 8a8 8 0 0 0 5.47 7.59c.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z"/>
                            </svg>
                        </a>
                        <button
                            class="theme-toggle"
                            type="button"
                            data-theme-toggle
                            aria-label="Toggle color theme"
                            title="Toggle theme"
                        >
                            <svg class="icon-moon" viewBox="0 0 16 16" aria-hidden="true">
                                <path d="M9.598 1.591a.749.749 0 0 1 .785-.175 7.001 7.001 0 1 1-8.967 8.967.75.75 0 0 1 .961-.96 5.5 5.5 0 0 0 7.046-7.046.75.75 0 0 1 .175-.786zm1.616 1.945a7 7 0 0 1-7.678 7.678 5.499 5.499 0 1 0 7.678-7.678z"/>
                            </svg>
                            <svg class="icon-sun" viewBox="0 0 16 16" aria-hidden="true">
                                <path d="M8 12a4 4 0 1 1 0-8 4 4 0 0 1 0 8zM8 0a.75.75 0 0 1 .75.75v1.5a.75.75 0 0 1-1.5 0V.75A.75.75 0 0 1 8 0zm0 13a.75.75 0 0 1 .75.75v1.5a.75.75 0 0 1-1.5 0v-1.5A.75.75 0 0 1 8 13zM2.343 2.343a.75.75 0 0 1 1.06 0l1.061 1.061a.75.75 0 0 1-1.06 1.06L2.343 3.404a.75.75 0 0 1 0-1.06zm9.193 9.193a.75.75 0 0 1 1.06 0l1.061 1.06a.75.75 0 0 1-1.06 1.061l-1.061-1.06a.75.75 0 0 1 0-1.061zM16 8a.75.75 0 0 1-.75.75h-1.5a.75.75 0 0 1 0-1.5h1.5A.75.75 0 0 1 16 8zM3 8a.75.75 0 0 1-.75.75H.75a.75.75 0 0 1 0-1.5h1.5A.75.75 0 0 1 3 8zm10.657-5.657a.75.75 0 0 1 0 1.06l-1.06 1.061a.75.75 0 1 1-1.061-1.06l1.06-1.061a.75.75 0 0 1 1.061 0zm-9.193 9.193a.75.75 0 0 1 0 1.06l-1.06 1.061a.75.75 0 0 1-1.061-1.06l1.06-1.061a.75.75 0 0 1 1.061 0z"/>
                            </svg>
                        </button>
                    </div>
                </header>

                <main id="main">
                    <section class="hero" aria-labelledby="hero-title">
                        <div class="hero-grid">
                            <div class="hero-copy">
                                <span class="eyebrow">"pre-v1 · in active development"</span>
                                <h1 id="hero-title">
                                    "Encrypted backups."
                                    <br/>
                                    <span class="accent">"Same everywhere."</span>
                                </h1>
                                <p class="lead">
                                    "FileFerry is an all-Rust backup CLI for operators, IT directors, and developers who want client-side encryption, predictable scripting, and boring restores on every machine they manage."
                                </p>
                                <div class="hero-actions">
                                    <a class="button primary" href="https://github.com/dunamismax/fileferry">
                                        <svg viewBox="0 0 16 16" aria-hidden="true">
                                            <path d="M8 0C3.58 0 0 3.58 0 8a8 8 0 0 0 5.47 7.59c.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0 0 16 8c0-4.42-3.58-8-8-8z"/>
                                        </svg>
                                        "View on GitHub"
                                    </a>
                                    <a class="button secondary" href="#status">"Read project status"</a>
                                </div>
                                <dl class="hero-facts" aria-label="Product facts">
                                    <div>
                                        <dt>"Command"</dt>
                                        <dd>"ferry"</dd>
                                    </div>
                                    <div>
                                        <dt>"Mode"</dt>
                                        <dd>"CLI-only"</dd>
                                    </div>
                                    <div>
                                        <dt>"Backends"</dt>
                                        <dd>"Local + S3"</dd>
                                    </div>
                                </dl>
                            </div>

                            <div class="terminal" aria-label="ferry command preview">
                                <div class="terminal-bar" aria-hidden="true">
                                    <span class="dot red"></span>
                                    <span class="dot amber"></span>
                                    <span class="dot green"></span>
                                    <span class="title">"~ · ferry"</span>
                                </div>
                                <pre><code inner_html=TERMINAL_HTML></code></pre>
                            </div>
                        </div>
                    </section>

                    <section class="section" id="status" aria-labelledby="status-title">
                        <div class="section-inner">
                            <div class="section-heading">
                                <h2 id="status-title">"Pre-v1 foundation, honest by default."</h2>
                                <p>
                                    "The Rust workspace, CLI shell, config and output contracts, initial crypto primitives, storage abstractions, and core backup, restore, and check primitives exist. The repository format is not yet frozen."
                                </p>
                            </div>
                            <div class="status-grid">
                                <article class="status-card">
                                    <span class="badge done">"Implemented"</span>
                                    <h3>"CLI foundation"</h3>
                                    <p>"version, completions, config precedence, profiles, JSON, JSONL, redacted diagnostics, golden tests."</p>
                                </article>
                                <article class="status-card">
                                    <span class="badge done">"Implemented"</span>
                                    <h3>"Crypto groundwork"</h3>
                                    <p>"master keys, passphrase key slots, HKDF subkeys, XChaCha20-Poly1305 authenticated envelopes."</p>
                                </article>
                                <article class="status-card">
                                    <span class="badge done">"Implemented"</span>
                                    <h3>"Storage groundwork"</h3>
                                    <p>"object-store trait, capability model, local backend, S3-compatible backend, retry/timeout policy."</p>
                                </article>
                                <article class="status-card">
                                    <span class="badge done">"Implemented"</span>
                                    <h3>"Backup pipeline"</h3>
                                    <p>"source walking, FastCDC chunking, zstd compression, encrypted chunks, indexes, and manifests."</p>
                                </article>
                                <article class="status-card">
                                    <span class="badge done">"Implemented"</span>
                                    <h3>"Restore + check"</h3>
                                    <p>"snapshot selection, path-scoped restore, destination safety, dry-run, deterministic check subsets."</p>
                                </article>
                                <article class="status-card">
                                    <span class="badge pending">"Not done yet"</span>
                                    <h3>"Retention + release"</h3>
                                    <p>"forget, prune, broader metadata application, signed cross-platform release artifacts and SBOMs."</p>
                                </article>
                            </div>
                        </div>
                    </section>

                    <section class="section" id="contract" aria-labelledby="contract-title">
                        <div class="section-inner">
                            <div class="section-heading">
                                <h2 id="contract-title">"Built for shells, logs, runbooks, and restore drills."</h2>
                                <p>
                                    "FileFerry is automation-first. Human progress is optional; stdout is always machine output and exit codes are part of the API."
                                </p>
                            </div>
                            <div class="contract-grid">
                                <article class="contract-card">
                                    <span class="num">"01"</span>
                                    <h3>"Stdout is data"</h3>
                                    <p>"Human progress and diagnostics stay on stderr so scripts can trust stdout."</p>
                                </article>
                                <article class="contract-card">
                                    <span class="num">"02"</span>
                                    <h3>"JSON and JSONL surfaces"</h3>
                                    <p>"Single-document output for state. Newline-delimited events for long operations."</p>
                                </article>
                                <article class="contract-card">
                                    <span class="num">"03"</span>
                                    <h3>"Dry-run destructive work"</h3>
                                    <p>"Forget, prune, and restore-over-existing have explicit, planned dry-run paths."</p>
                                </article>
                                <article class="contract-card">
                                    <span class="num">"04"</span>
                                    <h3>"Exit codes are part of the API"</h3>
                                    <p>"Failure families are documented and treated as automation contracts before v1."</p>
                                </article>
                            </div>
                        </div>
                    </section>

                    <section class="section" id="security" aria-labelledby="security-title">
                        <div class="section-inner">
                            <div class="section-heading">
                                <h2 id="security-title">"Encrypted before anything leaves the machine."</h2>
                                <p>
                                    "FileFerry is not trying to become a dashboard, daemon, scheduler, server, or compatibility shim. The product center is encrypted backup and reliable restore."
                                </p>
                            </div>
                            <div class="feature-grid">
                                <article class="feature-card">
                                    <span class="icon" aria-hidden="true">
                                        <svg viewBox="0 0 16 16"><path d="M4 4a4 4 0 1 1 8 0v2h.25c.97 0 1.75.78 1.75 1.75v5.5c0 .97-.78 1.75-1.75 1.75h-8.5A1.75 1.75 0 0 1 2 13.25v-5.5C2 6.78 2.78 6 3.75 6H4V4zm6.5 2V4a2.5 2.5 0 0 0-5 0v2h5zM3.5 7.75v5.5c0 .14.11.25.25.25h8.5a.25.25 0 0 0 .25-.25v-5.5a.25.25 0 0 0-.25-.25h-8.5a.25.25 0 0 0-.25.25z"/></svg>
                                    </span>
                                    <h3>"Client-side encryption"</h3>
                                    <p>"Contents, names, directory shape, manifests, indexes, and sensitive config are designed to stay encrypted."</p>
                                </article>
                                <article class="feature-card">
                                    <span class="icon" aria-hidden="true">
                                        <svg viewBox="0 0 16 16"><path d="M8 0a8 8 0 1 1 0 16A8 8 0 0 1 8 0zm3.78 6.22a.75.75 0 0 0-1.06-1.06L6.75 9.13 5.28 7.66a.75.75 0 0 0-1.06 1.06l2 2a.75.75 0 0 0 1.06 0l4.5-4.5z"/></svg>
                                    </span>
                                    <h3>"Restore first"</h3>
                                    <p>"Every backup feature is judged by whether it makes restore safer, clearer, and easier to verify."</p>
                                </article>
                                <article class="feature-card">
                                    <span class="icon" aria-hidden="true">
                                        <svg viewBox="0 0 16 16"><path d="M3.5 1.75a.25.25 0 0 1 .25-.25h6.5L13 4.25v9.5a.25.25 0 0 1-.25.25h-9a.25.25 0 0 1-.25-.25V1.75zM3.75 0A1.75 1.75 0 0 0 2 1.75v12.5C2 15.216 2.784 16 3.75 16h9A1.75 1.75 0 0 0 14.5 14.25V4a.75.75 0 0 0-.22-.53L11.03.22A.75.75 0 0 0 10.5 0h-6.75z"/></svg>
                                    </span>
                                    <h3>"Original repository format"</h3>
                                    <p>"FileFerry does not read or write restic, rustic, Borg, Kopia, or rclone-native repositories."</p>
                                </article>
                                <article class="feature-card">
                                    <span class="icon" aria-hidden="true">
                                        <svg viewBox="0 0 16 16"><path d="M1.5 8a6.5 6.5 0 1 1 13 0 6.5 6.5 0 0 1-13 0zM8 0a8 8 0 1 0 0 16A8 8 0 0 0 8 0zm.75 4.75a.75.75 0 0 0-1.5 0v3.5c0 .2.08.39.22.53l2 2a.75.75 0 1 0 1.06-1.06L8.75 7.94V4.75z"/></svg>
                                    </span>
                                    <h3>"Append-friendly objects"</h3>
                                    <p>"Immutable objects, retry-safe writes, explicit commit markers, and no required rename assumptions."</p>
                                </article>
                                <article class="feature-card">
                                    <span class="icon" aria-hidden="true">
                                        <svg viewBox="0 0 16 16"><path d="M11.28 6.78a.75.75 0 0 0-1.06-1.06L7.25 8.69 5.78 7.22a.75.75 0 0 0-1.06 1.06l2 2a.75.75 0 0 0 1.06 0l3.5-3.5zM16 8A8 8 0 1 1 0 8a8 8 0 0 1 16 0zm-1.5 0a6.5 6.5 0 1 0-13 0 6.5 6.5 0 0 0 13 0z"/></svg>
                                    </span>
                                    <h3>"Evidence before claims"</h3>
                                    <p>"Platform support waits for CI, tests, release artifacts, and observed behavior — not aspirational ticks."</p>
                                </article>
                                <article class="feature-card">
                                    <span class="icon" aria-hidden="true">
                                        <svg viewBox="0 0 16 16"><path d="M7.47 1.22a.75.75 0 0 1 1.06 0l3.25 3.25a.75.75 0 0 1-1.06 1.06L8.75 3.56V11a.75.75 0 0 1-1.5 0V3.56L5.28 5.53a.75.75 0 0 1-1.06-1.06l3.25-3.25zM2 13.75a.75.75 0 0 1 .75-.75h10.5a.75.75 0 0 1 0 1.5H2.75a.75.75 0 0 1-.75-.75z"/></svg>
                                    </span>
                                    <h3>"Self-hostable"</h3>
                                    <p>"One Rust binary, one config format, one encrypted repository — owned by the person running it."</p>
                                </article>
                            </div>
                        </div>
                    </section>

                    <section class="section" id="roadmap" aria-labelledby="roadmap-title">
                        <div class="section-inner">
                            <div class="section-heading">
                                <h2 id="roadmap-title">"What's next."</h2>
                                <p>
                                    "The current foundation is useful only if it leads to boring recovery. The public roadmap keeps that pressure visible."
                                </p>
                            </div>
                            <div class="roadmap-grid">
                                <article class="roadmap-item now">
                                    <div class="phase">"Now"</div>
                                    <h3>"Retention + maintenance"</h3>
                                    <p>"forget, prune two-phase safety, recoverable mark/sweep, retention policy plumbing into the CLI."</p>
                                </article>
                                <article class="roadmap-item next">
                                    <div class="phase">"Next"</div>
                                    <h3>"Broader metadata + S3 init"</h3>
                                    <p>"platform metadata application, xattrs, mode/ownership restore, S3-compatible repository bootstrap from the CLI."</p>
                                </article>
                                <article class="roadmap-item later">
                                    <div class="phase">"Then"</div>
                                    <h3>"v1 release"</h3>
                                    <p>"frozen format v0, cross-platform CI matrix, signed release artifacts, checksums, SBOMs, completions."</p>
                                </article>
                            </div>
                        </div>
                    </section>

                    <section class="section" aria-labelledby="release-title">
                        <div class="section-inner deploy-section">
                            <div class="deploy-copy">
                                <span class="eyebrow">"self-hosting"</span>
                                <h2 id="release-title">"This site is a separate marketing binary."</h2>
                                <p>
                                    "The homepage is served by an Axum binary rendering Leptos views on the server. No database, no client-side app bundle, and a simple health endpoint for reverse proxies."
                                </p>
                            </div>
                            <div class="deploy-card">
                                <div class="deploy-card-header">"fileferry-web · run anywhere"</div>
                                <dl class="deploy-rows">
                                    <dt>"Binary"</dt>
                                    <dd>"fileferry-web"</dd>
                                    <dt>"Default bind"</dt>
                                    <dd>"0.0.0.0:8080"</dd>
                                    <dt>"Override"</dt>
                                    <dd>"FILEFERRY_WEB_ADDR=127.0.0.1:8080"</dd>
                                    <dt>"Health"</dt>
                                    <dd>"GET /healthz → 200 ok"</dd>
                                </dl>
                            </div>
                        </div>
                    </section>
                </main>

                <footer>
                    <span class="footer-brand">"FileFerry"</span>
                    <span>"MIT licensed · built in Rust"</span>
                    <span class="footer-links">
                        <a href="https://github.com/dunamismax/fileferry">"GitHub"</a>
                        <a href="https://fileferry.app/">"fileferry.app"</a>
                    </span>
                </footer>

                <script src="/assets/theme.js" defer></script>
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

        assert!(html.contains("Encrypted backups."));
        assert!(html.contains("Pre-v1 foundation"));
        assert!(html.contains("fileferry-web"));
        assert!(html.contains("/assets/site.css"));
        assert!(html.contains("/assets/theme.js"));
        assert!(html.contains("data-theme-toggle"));
        assert!(html.contains("Toggle color theme"));
    }

    #[tokio::test]
    async fn routes_serve_homepage_health_css_and_theme_script() {
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
            .clone()
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

        let js = app
            .oneshot(
                Request::builder()
                    .uri("/assets/theme.js")
                    .body(body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(js.status(), StatusCode::OK);
        assert_eq!(
            js.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/javascript; charset=utf-8"
        );
    }
}

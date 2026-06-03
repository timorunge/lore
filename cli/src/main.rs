#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use clap::{CommandFactory, Parser};

use lore_cli::cli::args::{Cli, Command};
use lore_cli::cli::{
    make_prefixes, resolve_all_configs, resolve_stores, run_per_config, run_per_config_async,
};

/// Print output, applying width-capping for human-readable (non-JSON) modes.
fn print_output(text: &str, json: bool) {
    if json {
        println!("{text}");
    } else {
        println!(
            "{}",
            lore::fmt::cap_output(text, lore_cli::terminal::output_width())
        );
    }
}

/// Print a query result: stdout for data, `[i ]` on stderr for empty results.
fn print_query_result(result: &lore::query::QueryResult, json: bool) {
    if result.total == 0 && !json {
        let paint = lore_cli::terminal::stderr_painter();
        eprintln!("[{} ] {}", paint.blue("i"), result.formatted);
    } else {
        print_output(&result.formatted, json);
    }
}

fn main() {
    let log_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error,rmcp=off"));
    tracing_subscriber::fmt().with_env_filter(log_filter).init();

    let cli = Cli::parse();

    let paint = lore_cli::terminal::stderr_painter();
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[{} ] {e:#}", paint.red("-"));
            std::process::exit(1);
        }
    };
    if let Err(e) = rt.block_on(async {
        match cli.command {
            Command::Init { global } => {
                if global {
                    lore_cli::cli::init::init_global()
                } else {
                    lore_cli::cli::init::init(cli.config.into_iter().next())
                }
            }
            Command::Ingest {
                recreate,
                dry_run,
                force,
                quiet,
                source,
            } => {
                let configs = resolve_all_configs(cli.config)?;
                lore_cli::cli::ingest::ingest(
                    &configs,
                    recreate,
                    dry_run,
                    force,
                    quiet,
                    source.as_deref(),
                )
                .await
            }
            Command::Search {
                query,
                limit,
                offset,
                source,
                topic,
                author,
                lang,
                origin,
                kind,
                format,
                max_per_source,
                sort,
                reverse,
                json,
            } => {
                let stores = resolve_stores(cli.config)?;
                let mode = lore_cli::terminal::output_mode(json);
                let args = lore::query::SearchArgs {
                    query,
                    pagination: lore::query::Pagination::new(limit, offset),
                    source,
                    topic,
                    author,
                    lang,
                    origin,
                    kind,
                    format,
                    max_per_source,
                    sort: Some(sort.into()),
                    reverse: Some(reverse),
                };
                let result = lore::query::search(&stores, args, mode)?;
                print_query_result(&result, json);
                Ok(())
            }
            Command::Topics {
                limit,
                offset,
                topic,
                author,
                source,
                lang,
                origin,
                kind,
                format,
                sort,
                reverse,
                json,
            } => {
                let stores = resolve_stores(cli.config)?;
                let result = lore::query::list_topics(
                    &stores,
                    lore::query::TopicsArgs {
                        topic,
                        author,
                        source,
                        lang,
                        origin,
                        kind,
                        format,
                        sort: Some(sort.into()),
                        reverse: Some(reverse),
                        pagination: lore::query::Pagination::new(limit, offset),
                    },
                    lore_cli::terminal::output_mode(json),
                )?;
                print_query_result(&result, json);
                Ok(())
            }
            Command::Docs {
                limit,
                offset,
                source,
                topic,
                title,
                author,
                lang,
                origin,
                kind,
                format,
                sort,
                reverse,
                json,
            } => {
                let stores = resolve_stores(cli.config)?;
                let result = lore::query::list_documents(
                    &stores,
                    lore::query::DocsArgs {
                        pagination: lore::query::Pagination::new(limit, offset),
                        source,
                        topic,
                        title,
                        author,
                        lang,
                        origin,
                        kind,
                        format,
                        sort: Some(sort.into()),
                        reverse: Some(reverse),
                    },
                    lore_cli::terminal::output_mode(json),
                )?;
                print_query_result(&result, json);
                Ok(())
            }
            Command::Read {
                source,
                limit,
                offset,
                json,
                full,
                pager,
                no_pager,
            } => {
                let stores = resolve_stores(cli.config)?;
                let pagination = if full {
                    lore::query::Pagination::unlimited()
                } else {
                    lore::query::Pagination::new(limit, offset)
                };
                lore_cli::cli::read::read(lore_cli::cli::read::ReadOptions {
                    stores,
                    source,
                    pagination,
                    full,
                    mode: lore_cli::terminal::output_mode(json),
                    json,
                    pager: pager.as_deref(),
                    no_pager,
                })
            }
            Command::Info { json } => {
                let stores = resolve_stores(cli.config)?;
                let output = lore_cli::cli::info::info(&stores, json)?;
                print_output(&output, json);
                Ok(())
            }
            Command::Serve {
                transport,
                host,
                port,
                expose,
                token,
                watch,
                debounce,
            } => {
                let configs = resolve_all_configs(cli.config)?;
                lore_cli::serve::run(lore_cli::serve::ServeOptions {
                    configs: &configs,
                    transport: transport.into(),
                    host: &host,
                    port,
                    expose,
                    token,
                    watch,
                    watch_debounce: debounce,
                })
                .await
            }
            Command::Watch {
                debounce,
                interval,
                source,
            } => {
                let configs = resolve_all_configs(cli.config)?;
                let prefixes = make_prefixes(&configs);
                lore_cli::cli::watch::watch_all(
                    &configs,
                    &prefixes,
                    debounce,
                    interval,
                    source.as_deref(),
                )
                .await
            }
            Command::Preview {
                paths,
                limit,
                offset,
                chunks,
                json,
                pager,
                no_pager,
            } => {
                let ingest_config = cli
                    .config
                    .into_iter()
                    .next()
                    .map(|p| lore::config::load_config_with_hints(&p))
                    .transpose()?;
                lore_cli::cli::preview::preview(lore_cli::cli::preview::PreviewOptions {
                    paths: &paths,
                    config: ingest_config.as_ref(),
                    pagination: lore::query::Pagination::new(limit, offset),
                    mode: lore_cli::terminal::output_mode(json),
                    chunks,
                    json,
                    pager: pager.as_deref(),
                    no_pager,
                })
                .await
            }
            #[cfg(feature = "llm")]
            Command::Enrich {
                source,
                topic,
                force,
            } => {
                run_per_config_async(
                    cli.config,
                    "one or more enrichments failed",
                    async |rc, pfx| {
                        lore_cli::cli::enrich::enrich(
                            &rc.config,
                            &rc.config_path,
                            source.as_deref(),
                            topic.as_deref(),
                            force,
                            pfx,
                        )
                        .await
                    },
                )
                .await
            }
            Command::Status { json, remote } => {
                run_per_config_async(
                    cli.config,
                    "one or more status checks failed",
                    async |rc, pfx| {
                        let store_path = rc.config.store_dir(&rc.config_path);
                        lore_cli::cli::status::status(&store_path, &rc.config, json, remote, pfx)
                            .await
                    },
                )
                .await
            }
            Command::Completions { shell } => {
                let mut cmd = Cli::command();
                let name = cmd.get_name().to_owned();
                clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
                Ok(())
            }
            Command::Splash { loop_ } => lore_cli::cli::splash::run(loop_),
            Command::Maintain { action } => {
                use lore_cli::cli::args::MaintainAction;
                match action {
                    Some(MaintainAction::Clean { scope }) => lore_cli::cli::maintain::clean(scope),
                    other => {
                        let action = other.unwrap_or(MaintainAction::Check { json: false });
                        run_per_config(
                            cli.config,
                            "one or more maintenance operations failed",
                            |rc, pfx| {
                                let store_path = rc.config.store_dir(&rc.config_path);
                                match &action {
                                    MaintainAction::Check { json } => {
                                        lore_cli::cli::maintain::check(
                                            &store_path,
                                            lore_cli::terminal::output_mode(*json),
                                            pfx,
                                        )
                                    }
                                    MaintainAction::Repair { json } => {
                                        lore_cli::cli::maintain::repair(
                                            &store_path,
                                            &rc.config.store,
                                            lore_cli::terminal::output_mode(*json),
                                            pfx,
                                        )
                                    }
                                    MaintainAction::Compact => lore_cli::cli::maintain::compact(
                                        &store_path,
                                        &rc.config.store,
                                        pfx,
                                    ),
                                    MaintainAction::Clean { .. } => unreachable!(),
                                    MaintainAction::Health => lore_cli::cli::maintain::health(
                                        &rc.config_path,
                                        &store_path,
                                        lore_cli::terminal::output_mode(false),
                                        pfx,
                                    ),
                                }
                            },
                        )
                    }
                }
            }
        }
    }) {
        eprintln!("[{} ] {e:#}", paint.red("-"));
        rt.shutdown_timeout(std::time::Duration::from_secs(30));
        std::process::exit(1);
    }
    rt.shutdown_timeout(std::time::Duration::from_secs(30));
}

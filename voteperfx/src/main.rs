use std::env;
use std::sync::Arc;
// use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::SinkExt;
use grpc_client::YellowstoneGrpc;
use log::{error, info, warn};
use tokio::sync::{mpsc, RwLock};
use tokio_stream::StreamExt;
use yellowstone_grpc_proto::geyser::{
    CommitmentLevel, SubscribeRequest, 
    SubscribeRequestFilterTransactions, SubscribeRequestFilterBlocks,
    SubscribeRequestPing, subscribe_update::UpdateOneof,
};

use voteperfx::{
    Config, DashboardRenderer, PerformanceStats, VoteTracker,
    log_simple_transaction, print_help, init_logging,
    process_vote_transaction, process_finalized_block,
    Result, VoteMonitorError,
};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let simple_mode = args.contains(&"--simple".to_string());
    
    if args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
        print_help(&args[0]);
        return Ok(());
    }

    init_logging(simple_mode);

    let config = Config::load_or_default("config.toml").await;
    
    let grpc_url = config.grpc_url.clone();
    let vote_account = config.vote_account.clone();
    
    if grpc_url.is_empty() || vote_account.is_empty() {
        error!("missing required configuration in config.toml");
        error!("please ensure grpc_url and vote_account are set");
        return Err(VoteMonitorError::Config(
            "missing grpc_url or vote_account in config.toml".to_string()
        ));
    }

    info!("vote monitor starting...");
    info!("monitoring vote account: {}", vote_account);
    
    if config.performance_logging.enabled {
        info!("performance logging enabled: {}", config.performance_logging.describe_filters());
    } else {
        info!("performance logging disabled");
    }
    
    if simple_mode {
        info!("simple cli logging mode");
    } else {
        info!("interactive dashboard mode (press ctrl+c to quit)");
    }

    let grpc = YellowstoneGrpc::new(grpc_url, None);
    let client = grpc.build_client().await
        .map_err(|e| VoteMonitorError::GrpcConnection(format!("{:?}", e)))?;

    let subscribe_request = create_subscription_request(&vote_account);

    let (mut subscribe_tx, mut stream) = client
        .lock()
        .await
        .subscribe_with_request(Some(subscribe_request))
        .await
        .map_err(|e| VoteMonitorError::GrpcConnection(format!("{:?}", e)))?;

    info!("connected to gRPC stream, processing votes...");

    // create shared state with arc<rwlock<>> for better async performance
    // rwlock allows multiple concurrent readers
    let vote_tracker = Arc::new(RwLock::new(VoteTracker::new()));
    let stats = Arc::new(RwLock::new(PerformanceStats::new()));
    let config = Arc::new(config);

    // bounded channels for async communication with backpressure
    let (tx_sender, mut tx_receiver) = mpsc::channel(1000);
    let (block_sender, mut block_receiver) = mpsc::channel(1000);
    
    // channel for dashboard cleanup signal
    let (cleanup_tx, mut cleanup_rx) = mpsc::channel::<()>(1);

    // clone references for tasks (more efficient than cloning arcs repeatedly)
    let vote_tracker_tx = vote_tracker.clone();
    let vote_tracker_block = vote_tracker.clone();
    let stats_block = stats.clone();
    let stats_dashboard = stats.clone();
    let config_block = config.clone();
    let vote_account_tx = vote_account.clone();
    let vote_account_block = vote_account.clone();
    let vote_account_dashboard = vote_account.clone();

    let mut dashboard_renderer = if !simple_mode {
        Some(DashboardRenderer::new())
    } else {
        None
    };

    // get updates and routes them to appropriate channels
    let stream_task = tokio::spawn(async move {
        while let Some(message) = stream.next().await {
            match message {
                Ok(msg) => {
                    match msg.update_oneof {
                        Some(UpdateOneof::Transaction(sut)) => {
                            if let Err(e) = tx_sender.send(sut).await {
                                warn!("transaction channel closed: {}, stopping stream", e);
                                break;
                            }
                        }
                        Some(UpdateOneof::Block(sub)) => {
                            if let Err(e) = block_sender.send(sub).await {
                                warn!("block channel closed: {}, stopping stream", e);
                                break;
                            }
                        }
                        Some(UpdateOneof::Ping(_ping)) => {
                            // respond to ping to keep connection alive
                            let ping_response = SubscribeRequest {
                                ping: Some(SubscribeRequestPing { id: 1 }),
                                ..Default::default()
                            };
                            if let Err(e) = subscribe_tx.send(ping_response).await {
                                error!("failed to send ping response: {}", e);
                                break;
                            }
                            log::debug!("responded to ping");
                        }
                        _ => {} // ignore other update types
                    }
                }
                Err(error) => {
                    error!("grpc stream error: {:?}", error);
                    break;
                }
            }
        }
        info!("gRPC stream task completed");
    });

    // processes incoming vote transactions and adds them as pending votes
    let tx_task = tokio::spawn(async move {
        while let Some(tx_update) = tx_receiver.recv().await {
            let mut tracker = vote_tracker_tx.write().await;
            if let Err(e) = process_vote_transaction(tx_update, &vote_account_tx, &mut tracker).await {
                error!("error processing vote transaction: {}", e);
            }
        }
        info!("transaction processing task completed");
    });

    // processes finalized blocks and handles dashboard updates
    let dashboard_task = tokio::spawn(async move {
        let mut render_interval = tokio::time::interval(Duration::from_millis(500));
        
        loop {
            tokio::select! {
                // handle cleanup signal
                _ = cleanup_rx.recv() => {
                    if let Some(ref renderer) = dashboard_renderer {
                        if let Err(e) = renderer.cleanup_without_clear() {
                            error!("failed to cleanup dashboard: {}", e);
                        }
                    }
                    break;
                }
                
                Some(block_update) = block_receiver.recv() => {
                    let confirmed_votes = {
                        let mut tracker = vote_tracker_block.write().await;
                        match process_finalized_block(block_update, &vote_account_block, &mut tracker).await {
                            Ok(votes) => votes,
                            Err(e) => {
                                error!("error processing finalized block: {}", e);
                                continue;
                            }
                        }
                    };
                    
                    // update performance stats
                    if !confirmed_votes.is_empty() {
                        let mut stats_guard = stats_block.write().await;
                        for confirmed_vote in confirmed_votes {
                            if simple_mode {
                                log_simple_transaction(&stats_guard, &confirmed_vote).await;
                            }
                            
                            if let Err(e) = stats_guard.add_confirmed_vote_with_config(
                                confirmed_vote, 
                                &vote_account_block, 
                                &config_block.performance_logging
                            ).await {
                                error!("error saving performance event: {}", e);
                            }
                        }
                    }
                }
                
                // only in dashboard mode
                _ = render_interval.tick() => {
                    if let Some(ref mut renderer) = dashboard_renderer {
                        let stats_guard = stats_dashboard.read().await;
                        if let Err(e) = renderer.render(&stats_guard, &vote_account_dashboard).await {
                            error!("dashboard render error: {}", e);
                        }
                    }
                }
            }
        }
    });

    info!("all processing tasks started - monitoring vote performance...");

    tokio::select! {
        _ = stream_task => {
            info!("stream task completed");
        },
        _ = tx_task => {
            info!("transaction processing task completed");
        },
        _ = dashboard_task => {
            info!("dashboard task completed");
        },
        _ = tokio::signal::ctrl_c() => {
            info!("shutdown signal received, generating final statistics...");
            
            // send cleanup signal to dashboard task
            if cleanup_tx.send(()).await.is_err() {
                error!("failed to send cleanup signal to dashboard task");
            }
            
            // give dashboard task a moment to cleanup
            tokio::time::sleep(Duration::from_millis(100)).await;
            // fix me
            // print_final_statistics(&stats, &vote_account).await;
            
            info!("shutdown complete");
        }
    }
    
    Ok(())
}

/// create the grpc subscription request for vote transactions and finalized blocks
fn create_subscription_request(vote_account: &str) -> SubscribeRequest {
    SubscribeRequest {
        transactions: std::collections::HashMap::from([(
            "vote_transactions".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(true),
                failed: Some(true),
                signature: None,
                account_include: vec![vote_account.to_string()],
                account_exclude: vec![],
                account_required: vec![],
            },
        )]),
        blocks: std::collections::HashMap::from([(
            "finalized_blocks".to_string(),
            SubscribeRequestFilterBlocks {
                account_include: vec![vote_account.to_string()],
                include_transactions: Some(true),
                include_accounts: Some(false),
                include_entries: Some(false),
            },
        )]),
        // fix me
        commitment: Some(CommitmentLevel::Finalized.into()),
        ..Default::default()
    }
}

// async fn print_final_statistics(stats: &Arc<RwLock<PerformanceStats>>, vote_account: &str) {
//     let stats_guard = stats.read().await;
//     let efficiency = stats_guard.calculate_efficiency();
//     let session_duration = stats_guard.session_start.elapsed();
//     let vote_rate = stats_guard.calculate_vote_rate();
//     let avg_latency = stats_guard.calculate_session_avg_latency();
//     let low_latency_pct = stats_guard.calculate_low_latency_percentage();
    
//     info!("═══════════════════════════════════════════════════════════════");
//     info!("final statistics");
//     info!("═══════════════════════════════════════════════════════════════");
//     info!("vote account: {}", vote_account);
//     info!("session duration: {:.1} minutes", session_duration.as_secs_f64() / 60.0);
//     info!("perf summary:");
//     info!("   total votes: {}", stats_guard.total_transactions());
//     info!("   vote rate: {:.2} votes/sec", vote_rate);
//     info!("   tvc efficiency: {:.1}%", efficiency);
//     info!("   tvc earned: {}/{}", stats_guard.total_tvc_earned(), stats_guard.total_tvc_possible());
//     info!("   avg latency: {:.1} slots", avg_latency);
//     info!("   low latency rate: {:.1}% (≤2 slots)", low_latency_pct);
//     info!("performance breakdown:");
//     info!("   🟩 optimal (16 tvc): {} votes", stats_guard.optimal_votes());
//     info!("   🟨 good (12-15 tvc): {} votes", stats_guard.good_votes());
//     info!("   🟥 poor (<12 tvc): {} votes", stats_guard.poor_votes());
    
//     if !stats_guard.session_poor_votes.is_empty() {
//         warn!("{} poor performance events detected this session", stats_guard.session_poor_votes.len());
//         info!("check ./performance_issues/ for detailed logs");
//     } else {
//         info!("no poor performance events detected. pro mode");
//     }
    
//     info!("═══════════════════════════════════════════════════════════════");
// }

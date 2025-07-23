use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use chrono::{DateTime, Local, Utc};
use crossterm::style::Color;
use serde::{Deserialize, Serialize};
// use tokio::sync::mpsc;

use crate::config::PerformanceFilterConfig;
use crate::error::Result;

pub type Slot = u64;

pub const VOTE_CREDITS_GRACE_SLOTS: u8 = 2;
pub const VOTE_CREDITS_MAXIMUM_PER_SLOT: u8 = 16;

#[derive(Debug, Clone)]
pub struct ConfirmedVote {
    pub signature: String,
    pub voted_slot: Slot,
    pub finalized_slot: Slot,
    pub latency: u64,
    pub tvc_credits: u64,
    pub timestamp: DateTime<Local>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TvcPerformanceLevel {
    Optimal,   // 16 TVC
    Good,      // 12-15 TVC  
    Fair,      // 8-11 TVC
    Poor,      // 4-7 TVC
    Critical,  // 1-3 TVC
}

impl TvcPerformanceLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            TvcPerformanceLevel::Optimal => "optimal",
            TvcPerformanceLevel::Good => "good",
            TvcPerformanceLevel::Fair => "fair",
            TvcPerformanceLevel::Poor => "poor",
            TvcPerformanceLevel::Critical => "critical",
        }
    }
    
    pub fn color(&self) -> Color {
        match self {
            TvcPerformanceLevel::Optimal => Color::Green,
            TvcPerformanceLevel::Good => Color::Yellow,
            TvcPerformanceLevel::Fair => Color::Cyan,
            TvcPerformanceLevel::Poor => Color::Magenta,
            TvcPerformanceLevel::Critical => Color::Red,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PoorPerformanceEvent {
    pub timestamp: DateTime<Utc>,
    pub landed_slot: Slot,
    pub voted_slot: Slot,
    pub latency: u64,
    pub tvc_credits: u64,
    pub transaction_signature: String,
    pub vote_account: String,
    pub total_tvc_credits: u64,
    pub total_voted_slots: usize,
    pub tvc_multiplier: f64,
}

/// circular buffer for recent votes - more efficient than vecdeque
pub struct CircularVoteBuffer {
    votes: Vec<Option<ConfirmedVote>>,
    head: usize,
    tail: usize,
    size: usize,
    capacity: usize,
}

impl CircularVoteBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            votes: vec![None; capacity],
            head: 0,
            tail: 0,
            size: 0,
            capacity,
        }
    }
    
    pub fn push(&mut self, vote: ConfirmedVote) {
        self.votes[self.tail] = Some(vote);
        self.tail = (self.tail + 1) % self.capacity;
        
        if self.size < self.capacity {
            self.size += 1;
        } else {
            self.head = (self.head + 1) % self.capacity;
        }
    }
    
    pub fn len(&self) -> usize {
        self.size
    }
    
    pub fn iter(&self) -> impl Iterator<Item = &ConfirmedVote> {
        let mut idx = self.head;
        let mut count = 0;
        std::iter::from_fn(move || {
            if count >= self.size {
                return None;
            }
            let item = self.votes[idx].as_ref();
            idx = (idx + 1) % self.capacity;
            count += 1;
            item
        })
    }
}

#[derive(Debug)]
pub struct PerformanceStats {
    pub session_start: Instant,
    pub total_transactions: AtomicU64,
    
    pub total_tvc_earned: AtomicU64,
    pub total_tvc_possible: AtomicU64,
    
    pub optimal_votes: AtomicU64,    // 16 TVC
    pub good_votes: AtomicU64,       // 12-15 TVC  
    pub poor_votes: AtomicU64,       // <12 TVC
    pub low_latency_votes: AtomicU64, // latency <= 2 slots
    
    // memory usage with circular buffers
    pub recent_confirmed_votes: VecDeque<ConfirmedVote>, // kept for compatibility
    pub session_poor_votes: VecDeque<ConfirmedVote>,
    pub avg_latency_window: VecDeque<u64>,
    pub avg_latency_window_sum: AtomicU64,
    
    // current state
    pub current_finalized_slot: AtomicU64,
    pub last_confirmed_vote: Option<ConfirmedVote>,
    
    // session-wide
    pub total_latency_sum: AtomicU64,
    
    // implement batched event writer channel?
    // event_sender: Option<mpsc::Sender<PoorPerformanceEvent>>,
}

impl PerformanceStats {
    pub fn new() -> Self {
        Self {
            session_start: Instant::now(),
            total_transactions: AtomicU64::new(0),
            total_tvc_earned: AtomicU64::new(0),
            total_tvc_possible: AtomicU64::new(0),
            optimal_votes: AtomicU64::new(0),
            good_votes: AtomicU64::new(0),
            poor_votes: AtomicU64::new(0),
            low_latency_votes: AtomicU64::new(0),
            recent_confirmed_votes: VecDeque::with_capacity(20),
            session_poor_votes: VecDeque::with_capacity(50),
            avg_latency_window: VecDeque::with_capacity(20),
            avg_latency_window_sum: AtomicU64::new(0),
            current_finalized_slot: AtomicU64::new(0),
            last_confirmed_vote: None,
            total_latency_sum: AtomicU64::new(0),
            // event_sender: None,
        }
    }
    
    #[inline]
    pub fn add_confirmed_vote(&mut self, confirmed: ConfirmedVote) {
        // atomic operations for lock-free updates
        self.total_transactions.fetch_add(1, Ordering::Relaxed);
        self.total_tvc_earned.fetch_add(confirmed.tvc_credits, Ordering::Relaxed);
        self.total_tvc_possible.fetch_add(VOTE_CREDITS_MAXIMUM_PER_SLOT as u64, Ordering::Relaxed);
        self.current_finalized_slot.store(confirmed.finalized_slot, Ordering::Relaxed);
        self.total_latency_sum.fetch_add(confirmed.latency, Ordering::Relaxed);
        
        match confirmed.tvc_credits {
            16 => { self.optimal_votes.fetch_add(1, Ordering::Relaxed); },
            12..=15 => { self.good_votes.fetch_add(1, Ordering::Relaxed); },
            _ => { self.poor_votes.fetch_add(1, Ordering::Relaxed); },
        }
        
        if confirmed.latency <= 2 {
            self.low_latency_votes.fetch_add(1, Ordering::Relaxed);
        }
        
        self.recent_confirmed_votes.push_back(confirmed.clone());
        if self.recent_confirmed_votes.len() > 20 {
            self.recent_confirmed_votes.pop_front();
        }
        
        self.avg_latency_window.push_back(confirmed.latency);
        self.avg_latency_window_sum.fetch_add(confirmed.latency, Ordering::Relaxed);
        if self.avg_latency_window.len() > 20 {
            let removed = self.avg_latency_window.pop_front().unwrap();
            self.avg_latency_window_sum.fetch_sub(removed, Ordering::Relaxed);
        }
        
        // track poor performance for analysis
        if confirmed.tvc_credits < VOTE_CREDITS_MAXIMUM_PER_SLOT as u64 {
            self.session_poor_votes.push_back(confirmed.clone());
            if self.session_poor_votes.len() > 50 {
                self.session_poor_votes.pop_front();
            }
        }
        
        self.last_confirmed_vote = Some(confirmed);
    }

    pub async fn add_confirmed_vote_with_config(
        &mut self, 
        confirmed: ConfirmedVote, 
        vote_account: &str,
        filter_config: &PerformanceFilterConfig,
    ) -> Result<()> {
        self.add_confirmed_vote(confirmed.clone());
        
        if filter_config.enabled {
            let performance_level = categorize_tvc_performance(confirmed.tvc_credits);
            
            if filter_config.should_save_vote(confirmed.latency, confirmed.tvc_credits, performance_level) {
                let event = PoorPerformanceEvent {
                    timestamp: Utc::now(),
                    landed_slot: confirmed.finalized_slot,
                    voted_slot: confirmed.voted_slot,
                    latency: confirmed.latency,
                    tvc_credits: confirmed.tvc_credits,
                    transaction_signature: confirmed.signature.clone(),
                    vote_account: vote_account.to_string(),
                    total_tvc_credits: confirmed.tvc_credits,
                    total_voted_slots: 1,
                    tvc_multiplier: confirmed.tvc_credits as f64 / VOTE_CREDITS_MAXIMUM_PER_SLOT as f64,
                };
                
                save_performance_event(event, filter_config).await?;
            }
        }
        
        Ok(())
    }
    
    #[inline]
    pub fn calculate_efficiency(&self) -> f64 {
        let total_possible = self.total_tvc_possible.load(Ordering::Relaxed);
        if total_possible == 0 { return 100.0; }
        let total_earned = self.total_tvc_earned.load(Ordering::Relaxed);
        (total_earned as f64 / total_possible as f64) * 100.0
    }
    
    #[inline]
    pub fn calculate_missed_credits(&self) -> u64 {
        let total_possible = self.total_tvc_possible.load(Ordering::Relaxed);
        let total_earned = self.total_tvc_earned.load(Ordering::Relaxed);
        total_possible.saturating_sub(total_earned)
    }
    
    #[inline]
    pub fn calculate_vote_rate(&self) -> f64 {
        let elapsed = self.session_start.elapsed().as_secs_f64();
        if elapsed == 0.0 { return 0.0; }
        let total_tx = self.total_transactions.load(Ordering::Relaxed);
        total_tx as f64 / elapsed
    }
    
    #[inline]
    pub fn calculate_avg_latency(&self) -> f64 {
        if self.avg_latency_window.is_empty() { return 0.0; }
        let sum = self.avg_latency_window_sum.load(Ordering::Relaxed);
        sum as f64 / self.avg_latency_window.len() as f64
    }
    
    #[inline]
    pub fn calculate_low_latency_percentage(&self) -> f64 {
        let total_tx = self.total_transactions.load(Ordering::Relaxed);
        if total_tx == 0 { return 0.0; }
        let low_latency = self.low_latency_votes.load(Ordering::Relaxed);
        (low_latency as f64 / total_tx as f64) * 100.0
    }
    
    #[inline]
    pub fn calculate_session_avg_latency(&self) -> f64 {
        let total_tx = self.total_transactions.load(Ordering::Relaxed);
        if total_tx == 0 { return 0.0; }
        let latency_sum = self.total_latency_sum.load(Ordering::Relaxed);
        latency_sum as f64 / total_tx as f64
    }
    
    #[inline]
    pub fn get_performance_status(&self) -> (&'static str, Color) {
        let efficiency = self.calculate_efficiency();
        if efficiency >= 95.0 {
            ("optimal", Color::Green)
        } else if efficiency >= 85.0 {
            ("good", Color::Yellow)
        } else {
            ("poor", Color::Red)
        }
    }
    
    // getters for atomic fields
    pub fn total_transactions(&self) -> u64 {
        self.total_transactions.load(Ordering::Relaxed)
    }
    
    pub fn total_tvc_earned(&self) -> u64 {
        self.total_tvc_earned.load(Ordering::Relaxed)
    }
    
    pub fn total_tvc_possible(&self) -> u64 {
        self.total_tvc_possible.load(Ordering::Relaxed)
    }
    
    pub fn optimal_votes(&self) -> u64 {
        self.optimal_votes.load(Ordering::Relaxed)
    }
    
    pub fn good_votes(&self) -> u64 {
        self.good_votes.load(Ordering::Relaxed)
    }
    
    pub fn poor_votes(&self) -> u64 {
        self.poor_votes.load(Ordering::Relaxed)
    }
    
    pub fn low_latency_votes(&self) -> u64 {
        self.low_latency_votes.load(Ordering::Relaxed)
    }
    
    pub fn current_finalized_slot(&self) -> u64 {
        self.current_finalized_slot.load(Ordering::Relaxed)
    }
}

#[inline]
pub fn calculate_tvc_credits_from_latency(latency: u64) -> u64 {
    if latency <= VOTE_CREDITS_GRACE_SLOTS as u64 {
        VOTE_CREDITS_MAXIMUM_PER_SLOT as u64
    } else {
        let penalty = latency - (VOTE_CREDITS_GRACE_SLOTS as u64);
        match (VOTE_CREDITS_MAXIMUM_PER_SLOT as u64).checked_sub(penalty) {
            Some(credits) if credits > 0 => credits,
            _ => 1, // minimum 1 credit
        }
    }
}

#[inline]
pub fn calculate_tvc_credits(voted_slot: Slot, finalized_slot: Slot) -> (u64, u64) {
    let latency = finalized_slot.saturating_sub(voted_slot);
    let credits = calculate_tvc_credits_from_latency(latency);
    (latency, credits)
}

#[inline]
pub fn categorize_tvc_performance(tvc_credits: u64) -> TvcPerformanceLevel {
    match tvc_credits {
        16 => TvcPerformanceLevel::Optimal,
        12..=15 => TvcPerformanceLevel::Good,
        8..=11 => TvcPerformanceLevel::Fair,
        4..=7 => TvcPerformanceLevel::Poor,
        _ => TvcPerformanceLevel::Critical,
    }
}

/// batched event writer
pub struct BatchedEventWriter {
    buffer: Vec<PoorPerformanceEvent>,
    buffer_capacity: usize,
    flush_interval: std::time::Duration,
    last_flush: Instant,
}

impl BatchedEventWriter {
    pub fn new(buffer_capacity: usize, flush_interval_secs: u64) -> Self {
        Self {
            buffer: Vec::with_capacity(buffer_capacity),
            buffer_capacity,
            flush_interval: std::time::Duration::from_secs(flush_interval_secs),
            last_flush: Instant::now(),
        }
    }
    
    pub async fn add_event(&mut self, event: PoorPerformanceEvent) -> Result<()> {
        self.buffer.push(event);
        
        // flush if buffer is full or interval elapsed
        if self.buffer.len() >= self.buffer_capacity || 
           self.last_flush.elapsed() >= self.flush_interval {
            self.flush().await?;
        }
        
        Ok(())
    }
    
    pub async fn flush(&mut self) -> Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        
        // create directory if needed
        tokio::fs::create_dir_all("./performance_issues").await?;
        
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let filename = format!("./performance_issues/performance_issues_{}.json", today);
        
        // batch serialize all events
        let mut batch_json = String::with_capacity(self.buffer.len() * 256);
        for event in &self.buffer {
            batch_json.push_str(&serde_json::to_string(event)?);
            batch_json.push('\n');
        }
        
        // single atomic write
        use tokio::fs::OpenOptions;
        use tokio::io::AsyncWriteExt;
        
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&filename)
            .await?;
        
        file.write_all(batch_json.as_bytes()).await?;
        file.flush().await?;
        
        self.buffer.clear();
        self.last_flush = Instant::now();
        
        Ok(())
    }
}

async fn save_performance_event(
    event: PoorPerformanceEvent,
    filter_config: &PerformanceFilterConfig,
) -> Result<()> {
    let performance_level = categorize_tvc_performance(event.tvc_credits);
    
    if !filter_config.should_save_vote(event.latency, event.tvc_credits, performance_level) {
        return Ok(());
    }
    
    // for now, still do immediate write
    tokio::fs::create_dir_all("./performance_issues").await?;
    
    let today = Utc::now().format("%Y-%m-%d").to_string();
    let filename = format!("./performance_issues/performance_issues_{}.json", today);
    
    let json_line = format!("{}\n", serde_json::to_string(&event)?);
    
    use tokio::fs::OpenOptions;
    use tokio::io::AsyncWriteExt;
    
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&filename)
        .await?;
    
    file.write_all(json_line.as_bytes()).await?;
    file.flush().await?;
    
    Ok(())
}

pub fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn format_duration(duration: std::time::Duration) -> String {
    let total_secs = duration.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    
    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}
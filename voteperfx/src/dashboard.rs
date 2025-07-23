use std::collections::VecDeque;
use std::io::{self, Write};

use crossterm::{
    cursor::{self, Hide, Show},
    execute,
    style::{ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType, size},
};

use crate::performance::{PerformanceStats, ConfirmedVote, format_duration, format_number};
use crate::error::{Result, VoteMonitorError};

/// pre-allocated buffers
pub struct DashboardRenderer {
    output_buffer: String,
    previous_lines: Vec<String>,
    terminal_width: u16,
    terminal_height: u16,
}

impl DashboardRenderer {
    pub fn new() -> Self {
        let (width, height) = size().unwrap_or((80, 24));
        Self {
            output_buffer: String::with_capacity(8192), // pre-allocate
            previous_lines: Vec::with_capacity(50),
            terminal_width: width,
            terminal_height: height,
        }
    }

    pub async fn render(&mut self, stats: &PerformanceStats, vote_account: &str) -> Result<()> {
        let mut stdout = io::stdout();
        
        // hide cursor during rendering
        execute!(stdout, Hide)
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to hide cursor: {}", e)))?;
        
        // check terminal size changes
        if let Ok((new_width, new_height)) = size() {
            if new_width != self.terminal_width || new_height != self.terminal_height {
                self.terminal_width = new_width;
                self.terminal_height = new_height;
                self.previous_lines.clear(); // force full redraw on resize
            }
        }
        
        self.build_dashboard_content(stats, vote_account);
        
        // split output into lines
        let new_lines: Vec<String> = self.output_buffer.lines().map(String::from).collect();
        
        if self.previous_lines.is_empty() {
            // first render or after resize - clear and draw everything
            execute!(stdout, Clear(ClearType::All), cursor::MoveTo(0, 0))
                .map_err(|e| VoteMonitorError::Dashboard(format!("failed to clear screen: {}", e)))?;
            
            write!(stdout, "{}", self.output_buffer)
                .map_err(|e| VoteMonitorError::Dashboard(format!("failed to write output: {}", e)))?;
        } else {
            // only redraw changed lines
            for (i, (old_line, new_line)) in self.previous_lines.iter()
                .zip(new_lines.iter())
                .enumerate()
            {
                if old_line != new_line {
                    execute!(stdout, cursor::MoveTo(0, i as u16))
                        .map_err(|e| VoteMonitorError::Dashboard(format!("failed to move cursor: {}", e)))?;
                    
                    // clear to end of line to handle shorter new content
                    write!(stdout, "{}\x1b[K", new_line)
                        .map_err(|e| VoteMonitorError::Dashboard(format!("failed to write line: {}", e)))?;
                }
            }
            
            // handle case where new content has more lines
            if new_lines.len() > self.previous_lines.len() {
                for (i, new_line) in new_lines[self.previous_lines.len()..].iter().enumerate() {
                    let row = self.previous_lines.len() + i;
                    execute!(stdout, cursor::MoveTo(0, row as u16))
                        .map_err(|e| VoteMonitorError::Dashboard(format!("failed to move cursor: {}", e)))?;
                    
                    write!(stdout, "{}", new_line)
                        .map_err(|e| VoteMonitorError::Dashboard(format!("failed to write line: {}", e)))?;
                }
            }
            
            // clear any remaining lines if new content is shorter
            if new_lines.len() < self.previous_lines.len() {
                for i in new_lines.len()..self.previous_lines.len() {
                    execute!(stdout, cursor::MoveTo(0, i as u16))
                        .map_err(|e| VoteMonitorError::Dashboard(format!("failed to move cursor: {}", e)))?;
                    
                    write!(stdout, "\x1b[K") // clear line
                        .map_err(|e| VoteMonitorError::Dashboard(format!("failed to clear line: {}", e)))?;
                }
            }
        }
        
        stdout.flush()
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to flush output: {}", e)))?;
        
        self.previous_lines = new_lines;
        
        Ok(())
    }
    
    /// cleanup terminal state - before exiting
    pub fn cleanup(&self) -> Result<()> {
        let mut stdout = io::stdout();
        
        execute!(stdout, ResetColor)
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to reset color: {}", e)))?;
        
        // clear screen
        execute!(stdout, Clear(ClearType::All))
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to clear screen: {}", e)))?;
        
        execute!(stdout, cursor::MoveTo(0, 0))
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to move cursor: {}", e)))?;
        
        // ensure cursor is visible
        execute!(stdout, Show)
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to show cursor: {}", e)))?;
        
        // write a reset sequence to ensure terminal is in a good state
        write!(stdout, "\x1b[0m")?; // reset all attributes
        write!(stdout, "\x1b[?25h")?; // show cursor (backup)
        
        // flush to ensure all changes are applied
        stdout.flush()
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to flush output: {}", e)))?;
        
        Ok(())
    }
    
    /// cleanup terminal without clearing screen - preserves final output
    pub fn cleanup_without_clear(&self) -> Result<()> {
        let mut stdout = io::stdout();
        
        execute!(stdout, ResetColor)
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to reset color: {}", e)))?;
        
        // ensure cursor is visible
        execute!(stdout, Show)
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to show cursor: {}", e)))?;
        
        // write reset sequences to ensure terminal is in a good state
        write!(stdout, "\x1b[0m")?; // reset all attributes
        write!(stdout, "\x1b[?25h")?; // show cursor (backup)
        write!(stdout, "\n")?; // add newline for clean output
        
        // flush to ensure all changes are applied
        stdout.flush()
            .map_err(|e| VoteMonitorError::Dashboard(format!("failed to flush output: {}", e)))?;
        
        Ok(())
    }

    /// dashboard in memory
    fn build_dashboard_content(&mut self, stats: &PerformanceStats, vote_account: &str) {
        self.output_buffer.clear();
        
        self.add_header(vote_account);
        
        self.add_session_overview(stats);
        
        self.add_tvc_performance_chart(&stats.recent_confirmed_votes);
        
        self.add_efficiency_metrics(stats);
        
        self.add_latency_metrics(stats);
        
        self.add_performance_breakdown(stats);
        
        self.add_recent_performance(stats);
        
        self.add_poor_performance_tracking(stats);
        
        self.add_footer(stats);
    }

    fn add_header(&mut self, vote_account: &str) {
        self.output_buffer.push_str("═══════════════════════════════════════════════════════════════\n");
        self.output_buffer.push_str("performance monitor\n");
        self.output_buffer.push_str(&format!("vote account: {}\n", vote_account));
        self.output_buffer.push_str("═══════════════════════════════════════════════════════════════\n\n");
    }

    fn add_session_overview(&mut self, stats: &PerformanceStats) {
        let uptime = format_duration(stats.session_start.elapsed());
        let vote_rate = stats.calculate_vote_rate();
        
        self.output_buffer.push_str(&format!(
            "current slot: {:>12}      session Uptime: {:>15}\n",
            stats.current_finalized_slot(), uptime
        ));
        self.output_buffer.push_str(&format!(
            "total votes: {:>13}      vote rate: {:>8.3} votes/sec\n\n",
            stats.total_transactions(), vote_rate
        ));
    }

    fn add_tvc_performance_chart(&mut self, recent_votes: &VecDeque<ConfirmedVote>) {
        self.output_buffer.push_str("tvc performance (last 20 votes)\n");
        
        let chart_lines = create_tvc_chart(recent_votes);
        for line in chart_lines {
            self.output_buffer.push_str(&line);
            self.output_buffer.push('\n');
        }
        self.output_buffer.push('\n');
    }

    fn add_efficiency_metrics(&mut self, stats: &PerformanceStats) {
        let efficiency = stats.calculate_efficiency();
        let missed_credits = stats.calculate_missed_credits();
        
        self.output_buffer.push_str("tvc efficiency\n");
        self.output_buffer.push_str(&format!(
            "   earned:  {:>8} credits   possible: {:>8} credits\n",
            stats.total_tvc_earned(), 
            stats.total_tvc_possible()
        ));
        self.output_buffer.push_str(&format!(
            "   missed:  {:>8} credits   efficiency: {:>6.1}%\n\n",
            missed_credits, 
            efficiency
        ));
    }

    fn add_latency_metrics(&mut self, stats: &PerformanceStats) {
        let session_avg_latency = stats.calculate_session_avg_latency();
        let low_latency_percentage = stats.calculate_low_latency_percentage();
        
        self.output_buffer.push_str("vote latency metrics\n");
        self.output_buffer.push_str(&format!(
            "   session avg latency: {:>6.1} slots   low latency rate: {:>6.1}%\n",
            session_avg_latency, low_latency_percentage
        ));
        self.output_buffer.push_str(&format!(
            "   low latency votes:   {:>6} of {}   (≤2 slots)\n\n",
            stats.low_latency_votes(), stats.total_transactions()
        ));
    }

    fn add_performance_breakdown(&mut self, stats: &PerformanceStats) {
        let total_votes = stats.optimal_votes() + stats.good_votes() + stats.poor_votes();
        
        self.output_buffer.push_str("performance breakdown\n");
        
        if total_votes > 0 {
            let optimal_pct = (stats.optimal_votes() as f64 / total_votes as f64) * 100.0;
            let good_pct = (stats.good_votes() as f64 / total_votes as f64) * 100.0;
            let poor_pct = (stats.poor_votes() as f64 / total_votes as f64) * 100.0;
            
            self.output_buffer.push_str(&format!(
                "   🟩 optimal (16 TVC):    {:>4} votes ({:>4.1}%)\n",
                stats.optimal_votes(), optimal_pct
            ));
            self.output_buffer.push_str(&format!(
                "   🟨 good (12-15 TVC):    {:>4} votes ({:>4.1}%)\n",
                stats.good_votes(), good_pct
            ));
            self.output_buffer.push_str(&format!(
                "   🟥 poor (<12 TVC):      {:>4} votes ({:>4.1}%)\n",
                stats.poor_votes(), poor_pct
            ));
        } else {
            self.output_buffer.push_str("   waiting for votes...\n");
        }
        self.output_buffer.push('\n');
    }

    fn add_recent_performance(&mut self, stats: &PerformanceStats) {
        self.output_buffer.push_str("recent performance (last 30 votes)\n");
        
        let recent_votes: Vec<_> = stats.recent_confirmed_votes
            .iter()
            .rev()
            .take(30)
            .collect();
        
        if recent_votes.is_empty() {
            self.output_buffer.push_str("   waiting for confirmed votes...\n");
        } else {
            for vote in recent_votes.iter().take(10) { // show top 10 for space
                let performance_icon = match vote.tvc_credits {
                    16 => "🟩",
                    12..=15 => "🟨", 
                    _ => "🟥",
                };
                
                let tvc_lost = 16u64.saturating_sub(vote.tvc_credits);
                let loss_text = if tvc_lost > 0 {
                    format!("(-{})", tvc_lost)
                } else {
                    "✅".to_string()
                };
                
                self.output_buffer.push_str(&format!(
                    "   {} slot {:>9} -> lat:{:>2} -> {:>2} tvc {} | tx: https://solscan.io/tx/{} \n",
                    performance_icon,
                    vote.voted_slot,
                    vote.latency,
                    vote.tvc_credits,
                    loss_text,
                    vote.signature
                ));
            }
            
            let total_recent = recent_votes.len() as f64;
            let avg_recent_latency = recent_votes.iter().map(|v| v.latency).sum::<u64>() as f64 / total_recent;
            let total_tvc_lost: u64 = recent_votes.iter().map(|v| 16u64.saturating_sub(v.tvc_credits)).sum();
            let optimal_count = recent_votes.iter().filter(|v| v.tvc_credits == 16).count();
            let optimal_percentage = (optimal_count as f64 / total_recent) * 100.0;
            
            self.output_buffer.push_str(&format!(
                "\n   recent summary: avg latency {:.1}, {} tvc lost, {:.1}% optimal ({}/{})\n",
                avg_recent_latency, total_tvc_lost, optimal_percentage, optimal_count, recent_votes.len()
            ));
        }
        self.output_buffer.push('\n');
    }

    fn add_poor_performance_tracking(&mut self, stats: &PerformanceStats) {
        self.output_buffer.push_str("poor performance events (< 16 tvc)\n");
        
        let poor_votes: Vec<_> = stats.session_poor_votes
            .iter()
            .rev()
            .take(15)
            .collect();
        
        if poor_votes.is_empty() {
            self.output_buffer.push_str("   no poor performance votes in session\n");
        } else {
            for vote in poor_votes {
                let severity = match vote.tvc_credits {
                    12..=15 => "🟨",
                    8..=11 => "🟧", 
                    4..=7 => "🟥",
                    _ => "💀",
                };
                
                self.output_buffer.push_str(&format!(
                    "   {} slot {:>9} -> lat:{:>2} -> {:>2} tvc | tx: https://solscan.io/tx/{} \n",
                    severity,
                    vote.voted_slot,
                    vote.latency,
                    vote.tvc_credits,
                    vote.signature
                ));
            }
        }
        self.output_buffer.push('\n');
    }

    fn add_footer(&mut self, stats: &PerformanceStats) {
        let (status_text, _status_color) = stats.get_performance_status();
        
        self.output_buffer.push_str(&format!("status: {} performance\n", status_text));
        self.output_buffer.push_str("═══════════════════════════════════════════════════════════════\n");
        self.output_buffer.push_str("press ctrl+c to quit\n");
    }
}

impl Drop for DashboardRenderer {
    fn drop(&mut self) {
        // best effort cleanup - ignore errors on drop
        let _ = self.cleanup();
    }
}

fn create_tvc_chart(recent_votes: &VecDeque<ConfirmedVote>) -> Vec<String> {
    const BAR_HEIGHT: usize = 4;
    const BAR_WIDTH: usize = 20;
    
    // tvc with padding
    let mut tvc_values = Vec::with_capacity(BAR_WIDTH);
    let last_votes: Vec<_> = recent_votes.iter().rev().take(BAR_WIDTH).collect();
    
    for vote in last_votes.iter().rev() {
        tvc_values.push(vote.tvc_credits);
    }
    
    // pad with zeros if we have fewer votes
    while tvc_values.len() < BAR_WIDTH {
        tvc_values.insert(0, 0);
    }
    
    let mut chart_lines = Vec::with_capacity(BAR_HEIGHT + 2);
    
    // build chart from top to bottom - static strings
    for level in (1..=BAR_HEIGHT).rev() {
        let mut line = String::with_capacity(64);
        line.push_str(&format!("{:2} |", level * 4));
        
        for &tvc in &tvc_values {
            let bar_height = match tvc {
                0 => 0,
                1..=4 => 1,
                5..=8 => 2,
                9..=12 => 3,
                13..=16 => 4,
                _ => 4,
            };
            
            if bar_height >= level {
                let bar_char = match tvc {
                    16 => "\x1b[32m▓\x1b[0m",      // full performance - green
                    12..=15 => "\x1b[38;5;208m▓\x1b[0m", // good performance - orange
                    _ => "\x1b[31m▓\x1b[0m",       // poor performance - red
                };
                line.push(' ');
                line.push_str(bar_char);
            } else {
                line.push_str("  ");
            }
        }
        chart_lines.push(line);
    }
    
    let mut baseline = String::with_capacity(64);
    baseline.push_str(" 0 |");
    for _ in 0..BAR_WIDTH {
        baseline.push_str("──");
    }
    chart_lines.push(baseline);
    
    chart_lines
}

pub async fn render_dashboard_with_colors(stats: &PerformanceStats, vote_account: &str) -> Result<()> {
    let mut stdout = io::stdout();
    
    execute!(stdout, Hide, Clear(ClearType::All), cursor::MoveTo(0, 0))
        .map_err(|e| VoteMonitorError::Dashboard(format!("terminal error: {}", e)))?;
    
    let efficiency = stats.calculate_efficiency();
    let (status_text, status_color) = stats.get_performance_status();
    
    println!("═══════════════════════════════════════════════════════════════");
    println!("solana vote monitor");
    println!("vote account: {}", vote_account);
    println!("═══════════════════════════════════════════════════════════════\n");
    
    execute!(stdout, SetForegroundColor(status_color))?;
    println!("status: {} performance ({:.1}% efficiency)", status_text, efficiency);
    execute!(stdout, ResetColor)?;
    
    println!("total votes: {} | uptime: {}", 
             format_number(stats.total_transactions()),
             format_duration(stats.session_start.elapsed()));
    
    stdout.flush()
        .map_err(|e| VoteMonitorError::Dashboard(format!("flush error: {}", e)))?;
    
    Ok(())
}

pub async fn render_simple_dashboard(stats: &PerformanceStats, vote_account: &str) -> Result<()> {
    let efficiency = stats.calculate_efficiency();
    let uptime = format_duration(stats.session_start.elapsed());
    let vote_rate = stats.calculate_vote_rate();
    
    println!("=== solana vote monitor ===");
    println!("vote account: {}", vote_account);
    println!("session uptime: {} | total votes: {} | rate: {:.2}/sec", 
             uptime, stats.total_transactions(), vote_rate);
    println!("tvc efficiency: {:.1}% ({}/{} credits)", 
             efficiency, stats.total_tvc_earned(), stats.total_tvc_possible());
    println!("performance: {} optimal, {} good, {} poor votes",
             stats.optimal_votes(), stats.good_votes(), stats.poor_votes());
    
    if let Some(last_vote) = &stats.last_confirmed_vote {
        println!("last vote: slot {} → {} tvc (latency: {})", 
                 last_vote.voted_slot, last_vote.tvc_credits, last_vote.latency);
    }
    
    println!("=====================================\n");
    
    Ok(())
}
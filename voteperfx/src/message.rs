use crate::performance::{ConfirmedVote, PoorPerformanceEvent, Slot};
use crate::vote_tracker::{PendingVote, VoteTrackerStats};
use tokio::sync::oneshot;

#[derive(Debug)]
pub enum VoteCommand {
    AddPending(PendingVote),
    ConfirmVote {
        signature: String,
        voted_slot: Slot,
        finalized_slot: Slot,
        respond_to: oneshot::Sender<Option<ConfirmedVote>>,
    },
    MarkSlotProcessed(Slot),
    HasProcessedSlot {
        slot: Slot,
        respond_to: oneshot::Sender<bool>,
    },
    GetStats {
        respond_to: oneshot::Sender<VoteTrackerStats>,
    },
    Cleanup,
}

#[derive(Debug)]
pub enum StatsCommand {
    AddConfirmedVote {
        vote: ConfirmedVote,
        vote_account: String,
    },
    GetEfficiency {
        respond_to: oneshot::Sender<f64>,
    },
    GetVoteRate {
        respond_to: oneshot::Sender<f64>,
    },
    GetAvgLatency {
        respond_to: oneshot::Sender<f64>,
    },
    GetLowLatencyPercentage {
        respond_to: oneshot::Sender<f64>,
    },
    GetSessionAvgLatency {
        respond_to: oneshot::Sender<f64>,
    },
    GetPerformanceStatus {
        respond_to: oneshot::Sender<(String, crossterm::style::Color)>,
    },
    GetRecentVotes {
        respond_to: oneshot::Sender<Vec<ConfirmedVote>>,
    },
    GetPoorVotes {
        respond_to: oneshot::Sender<Vec<ConfirmedVote>>,
    },
    GetCurrentSlot {
        respond_to: oneshot::Sender<Slot>,
    },
    GetTotals {
        respond_to: oneshot::Sender<(u64, u64, u64, u64, u64)>, // transactions, tvc_earned, tvc_possible, optimal, good, poor
    },
}

#[derive(Debug, Clone)]
pub enum SystemEvent {
    VoteAdded(PendingVote),
    VoteConfirmed(ConfirmedVote),
    VoteMissed {
        signature: String,
        reason: String,
    },
    PerformanceEvent(PoorPerformanceEvent),
    SlotProcessed(Slot),
    CleanupCompleted {
        remaining_votes: usize,
    },
}

#[derive(Debug)]
pub enum Message {
    VoteCommand(VoteCommand),
    StatsCommand(StatsCommand),
    Event(SystemEvent),
    Shutdown,
}
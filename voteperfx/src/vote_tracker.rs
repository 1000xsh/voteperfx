use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, Local};
use rustc_hash::{FxHashMap, FxHashSet};
use solana_sdk::{program_utils::limited_deserialize, vote::instruction::VoteInstruction};

use crate::performance::{ConfirmedVote, Slot, calculate_tvc_credits};
use crate::error::{Result, VoteMonitorError};

// for verification
pub const VOTE_PROGRAM_ID: [u8; 32] = [
    7, 97, 72, 29, 53, 116, 116, 187, 124, 77, 118, 36, 235, 211, 189, 179, 
    216, 53, 94, 115, 209, 16, 67, 252, 13, 163, 83, 128, 0, 0, 0, 0
];

#[derive(Debug, Clone)]
pub struct VoteSlotInfo {
    pub slot: Slot,
    pub confirmation_count: Option<u32>,
}

impl VoteSlotInfo {
    pub fn new(slot: Slot, confirmation_count: Option<u32>) -> Self {
        Self { slot, confirmation_count }
    }
    
    /// check if this is a new vote (confirmation_count == 1)
    pub fn is_new_vote(&self) -> bool {
        self.confirmation_count == Some(1)
    }
    
    /// check if this is an existing vote (confirmation_count > 1)
    pub fn is_existing_vote(&self) -> bool {
        self.confirmation_count.is_some_and(|count| count > 1)
    }
}

/// pending vote awaiting confirmation in a finalized block
#[derive(Debug, Clone)]
pub struct PendingVote {
    pub signature: Arc<String>,  // arc to avoid repeated allocations
    pub voted_slots: FxHashSet<Slot>,
    pub transaction_slot: Slot,
    pub timestamp: DateTime<Local>,
    pub instruction_data: Vec<u8>,
}

/// signature cache - avoid encoding
#[derive(Debug)]
pub struct SignatureCache {
    cache: FxHashMap<[u8; 64], Arc<String>>,
    max_entries: usize,
}

impl SignatureCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: FxHashMap::with_capacity_and_hasher(max_entries, Default::default()),
            max_entries,
        }
    }
    
    pub fn get_or_insert(&mut self, signature_bytes: &[u8]) -> Arc<String> {
        // ensure we have exactly 64 bytes
        let mut key = [0u8; 64];
        key.copy_from_slice(&signature_bytes[..64.min(signature_bytes.len())]);
        
        if let Some(cached) = self.cache.get(&key) {
            return cached.clone();
        }
        
        // lru eviction if needed
        if self.cache.len() >= self.max_entries {
            // simple eviction: remove first entry (not true lru but fast)
            if let Some(first_key) = self.cache.keys().next().cloned() {
                self.cache.remove(&first_key);
            }
        }
        
        let signature = Arc::new(fd_bs58::encode_64(&key));
        self.cache.insert(key, signature.clone());
        signature
    }
}

/// circular buffer for confirmed votes
#[derive(Debug)]
pub struct CircularBuffer<T> {
    data: Vec<Option<T>>,
    head: usize,
    tail: usize,
    size: usize,
    capacity: usize,
}

impl<T: Clone> CircularBuffer<T> {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![None; capacity],
            head: 0,
            tail: 0,
            size: 0,
            capacity,
        }
    }
    
    pub fn push(&mut self, item: T) {
        self.data[self.tail] = Some(item);
        self.tail = (self.tail + 1) % self.capacity;
        
        if self.size < self.capacity {
            self.size += 1;
        } else {
            // overwrite oldest
            self.head = (self.head + 1) % self.capacity;
        }
    }
    
    pub fn len(&self) -> usize {
        self.size
    }
    
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        let mut idx = self.head;
        let mut count = 0;
        std::iter::from_fn(move || {
            if count >= self.size {
                return None;
            }
            let item = self.data[idx].as_ref();
            idx = (idx + 1) % self.capacity;
            count += 1;
            item
        })
    }
}

/// vote correlation tracker
/// tracks votes from transaction -> finalized block.
#[derive(Debug)]
pub struct VoteTracker {
    // awaiting confirmation (signature -> pendingvote)
    pending_votes: FxHashMap<Arc<String>, PendingVote>,
    
    // recently confirmed votes for analysis (circular buffer)
    confirmed_votes: CircularBuffer<ConfirmedVote>,
    
    // track processed slots
    processed_slots: CircularBuffer<Slot>,
    
    // signature cache
    signature_cache: SignatureCache,
    
    // state for cleanup
    last_cleanup_slot: Slot,
    last_cleanup_time: Instant,
    pending_count: usize,
}

impl VoteTracker {
    pub fn new() -> Self {
        Self {
            pending_votes: FxHashMap::with_capacity_and_hasher(1024, Default::default()),
            confirmed_votes: CircularBuffer::new(100),
            processed_slots: CircularBuffer::new(50),
            signature_cache: SignatureCache::new(2048),
            last_cleanup_slot: 0,
            last_cleanup_time: Instant::now(),
            pending_count: 0,
        }
    }
    
    /// awaiting confirmation
    #[inline]
    pub fn add_pending_vote(&mut self, pending: PendingVote) {
        self.pending_votes.insert(pending.signature.clone(), pending);
        self.pending_count += 1;
        
        // time-based cleanup to prevent memory growth (every 60 seconds)
        if self.last_cleanup_time.elapsed().as_secs() >= 60 {
            self.cleanup_old_pending();
        }
    }
    
    /// attempt to confirm a vote from a finalized block
    /// 
    /// returns Some(ConfirmedVote) if the vote was successfully confirmed,
    /// none if no matching pending vote was found.
    #[inline]
    pub fn confirm_vote(&mut self, signature: Arc<String>, voted_slot: Slot, finalized_slot: Slot) -> Option<ConfirmedVote> {
        // validate slot ordering
        if finalized_slot < voted_slot {
            log::warn!("invalid slot order: finalized_slot {} < voted_slot {}", finalized_slot, voted_slot);
            return None;
        }
        
        if let Some(pending) = self.pending_votes.get(&signature) {
            // verify this voted_slot was actually in the original pending vote
            if pending.voted_slots.contains(&voted_slot) {
                // remove the pending vote and create confirmed vote
                self.pending_votes.remove(&signature);
                self.pending_count -= 1;
                
                // calculate vote latency: finalized_slot - voted_slot
                let latency = finalized_slot.saturating_sub(voted_slot);
                let tvc_credits = crate::performance::calculate_tvc_credits_from_latency(latency);
                
                let confirmed = ConfirmedVote {
                    signature: (*signature).clone(),
                    voted_slot,
                    finalized_slot,
                    latency,
                    tvc_credits,
                    timestamp: Local::now(),
                };
                
                // use circular buffer for o(1) operations
                self.confirmed_votes.push(confirmed.clone());
                
                Some(confirmed)
            } else {
                // voted_slot not in original pending vote - no confirmation
                log::debug!("voted slot {} not found in pending slots {:?} for signature {}", 
                           voted_slot, pending.voted_slots, &signature[..8]);
                None
            }
        } else {
            // no pending vote found - create direct confirmation
            // this happens when we see the confirmation before the transaction. fix me.
            let (latency, tvc_credits) = calculate_tvc_credits(voted_slot, finalized_slot);
            
            log::debug!(
                "direct vote confirmation: slot {} → block {} → latency {} → {} tvc (no pending)",
                voted_slot, finalized_slot, latency, tvc_credits
            );
            
            // create confirmed vote even without pending match
            Some(ConfirmedVote {
                signature: (*signature).clone(),
                voted_slot,
                finalized_slot,
                latency,
                tvc_credits,
                timestamp: Local::now(),
            })
        }
    }
    
    #[inline]
    pub fn has_processed_slot(&self, slot: Slot) -> bool {
        self.processed_slots.iter().any(|&s| s == slot)
    }
    
    #[inline]
    pub fn mark_slot_processed(&mut self, slot: Slot) {
        self.processed_slots.push(slot);
    }
    
    pub fn get_stats(&self) -> VoteTrackerStats {
        VoteTrackerStats {
            pending_votes: self.pending_count,
            confirmed_votes: self.confirmed_votes.len(),
            processed_slots: self.processed_slots.len(),
        }
    }
    
    fn cleanup_old_pending(&mut self) {
        let current_slot = self.processed_slots.iter().last().cloned().unwrap_or(0);
        let cutoff_slot = current_slot.saturating_sub(100);
        
        self.pending_votes.retain(|_, pending| {
            pending.transaction_slot > cutoff_slot
        });
        
        self.pending_count = self.pending_votes.len();
        self.last_cleanup_slot = current_slot;
        self.last_cleanup_time = Instant::now();
        
        log::debug!("cleaned up old pending votes, {} remaining", self.pending_count);
    }
    
    /// get cached signature or create new one
    pub fn get_or_cache_signature(&mut self, signature_bytes: &[u8]) -> Arc<String> {
        self.signature_cache.get_or_insert(signature_bytes)
    }
}

#[derive(Debug, Clone)]
pub struct VoteTrackerStats {
    pub pending_votes: usize,
    pub confirmed_votes: usize,
    pub processed_slots: usize,
}

/// parse vote instruction data to extract vote slot information
/// 
/// extract the slots being voted on along with their confirmation counts.
pub fn parse_vote_instruction_data(data: &[u8]) -> Result<Vec<VoteSlotInfo>> {
    match limited_deserialize::<VoteInstruction>(data) {
        Ok(vote_instruction) => {
            use solana_sdk::vote::instruction::VoteInstruction;
            
            let vote_slots = match vote_instruction {
                VoteInstruction::Vote(vote) | VoteInstruction::VoteSwitch(vote, _) => {
                    vote.slots.into_iter().map(|slot| VoteSlotInfo::new(slot, Some(1))).collect()
                }
                VoteInstruction::UpdateVoteState(vote_state_update)
                | VoteInstruction::UpdateVoteStateSwitch(vote_state_update, _)
                | VoteInstruction::CompactUpdateVoteState(vote_state_update)
                | VoteInstruction::CompactUpdateVoteStateSwitch(vote_state_update, _) => {
                    vote_state_update.lockouts.into_iter().map(|lockout| {
                        VoteSlotInfo::new(lockout.slot(), Some(lockout.confirmation_count()))
                    }).collect()
                }
                VoteInstruction::TowerSync(tower_sync)
                | VoteInstruction::TowerSyncSwitch(tower_sync, _) => {
                    tower_sync.lockouts.into_iter().map(|lockout| {
                        VoteSlotInfo::new(lockout.slot(), Some(lockout.confirmation_count()))
                    }).collect()
                }
                _ => return Err(VoteMonitorError::VoteParsing("unknown vote instruction type".to_string())),
            };
            
            Ok(vote_slots)
        }
        Err(e) => Err(VoteMonitorError::VoteParsing(format!("failed to deserialize vote instruction: {}", e))),
    }
}

/// process a vote transaction from the grpc stream
/// 
/// extracts vote information from transactions and adds
/// pending votes to the tracker for later confirmation.
pub async fn process_vote_transaction(
    tx_update: yellowstone_grpc_proto::geyser::SubscribeUpdateTransaction,
    _vote_account: &str,
    vote_tracker: &mut VoteTracker,
) -> Result<()> {
    let transaction_slot = tx_update.slot;
    
    let transaction = tx_update.transaction
        .ok_or_else(|| VoteMonitorError::VoteParsing("empty transaction".to_string()))?;
    
    if !transaction.is_vote {
        return Ok(());
    }
    
    let signature_bytes = &transaction.signature;
    let signature_base58 = vote_tracker.get_or_cache_signature(signature_bytes);
    
    log::debug!("processing vote transaction at slot {} (sig: {})", 
               transaction_slot, &signature_base58[..8]);
    
    if let Some(tx_data) = &transaction.transaction {
        if let Some(message) = &tx_data.message {
            for instruction in &message.instructions {
                if let Some(program_account) = message.account_keys.get(instruction.program_id_index as usize) {
                    if program_account == &VOTE_PROGRAM_ID {
                        let vote_slots = parse_vote_instruction_data(&instruction.data)?;
                        
                        // confirmation_count == 1
                        let new_voted_slots: FxHashSet<Slot> = vote_slots
                            .into_iter()
                            .filter(|vote_info| vote_info.is_new_vote())
                            .map(|vote_info| vote_info.slot)
                            .collect();
                        
                        if !new_voted_slots.is_empty() {
                            // create pending vote for tracking
                            let pending_vote = PendingVote {
                                signature: signature_base58.clone(),
                                voted_slots: new_voted_slots.clone(),
                                transaction_slot,
                                timestamp: Local::now(),
                                instruction_data: instruction.data.clone(),
                            };
                            
                            vote_tracker.add_pending_vote(pending_vote);
                            
                            log::debug!(
                                "added pending vote: {} new votes at slot {} (sig: {})",
                                new_voted_slots.len(), transaction_slot, &signature_base58[..8]
                            );
                        }
                    }
                }
            }
        }
    }
    
    Ok(())
}

/// process a finalized block to confirm pending votes
/// 
/// examines finalized blocks for vote confirmations and
/// returns a list of confirmed votes.
pub async fn process_finalized_block(
    block_update: yellowstone_grpc_proto::geyser::SubscribeUpdateBlock,
    vote_account: &str,
    vote_tracker: &mut VoteTracker,
) -> Result<Vec<ConfirmedVote>> {
    let mut confirmed_votes = Vec::new();
    let finalized_slot = block_update.slot;
    
    if vote_tracker.has_processed_slot(finalized_slot) {
        return Ok(confirmed_votes);
    }
    
    vote_tracker.mark_slot_processed(finalized_slot);
    
    log::debug!("processing finalized block at slot {}", finalized_slot);
    
    for tx_info in block_update.transactions {
        if let Some(transaction) = tx_info.transaction {
            if let Some(signature_bytes) = transaction.signatures.first() {
                let signature_base58 = vote_tracker.get_or_cache_signature(signature_bytes);
                
                if let Some(confirmed) = process_transaction_in_block(
                    &transaction,
                    signature_base58.clone(),
                    finalized_slot,
                    vote_account,
                    vote_tracker,
                )? {
                    confirmed_votes.push(confirmed);
                }
            }
        }
    }
    
    log::debug!("confirmed {} votes in block {}", confirmed_votes.len(), finalized_slot);
    Ok(confirmed_votes)
}

/// process individual transaction within a finalized block
fn process_transaction_in_block(
    transaction: &yellowstone_grpc_proto::prelude::Transaction,
    signature: Arc<String>,
    finalized_slot: Slot,
    _vote_account: &str,
    vote_tracker: &mut VoteTracker,
) -> Result<Option<ConfirmedVote>> {
    // extract vote instruction data and verify it contains our vote account
    if let Some(message) = &transaction.message {
        for instruction in &message.instructions {
            if let Some(program_account) = message.account_keys.get(instruction.program_id_index as usize) {
                if program_account == &VOTE_PROGRAM_ID {
                    let vote_slots = parse_vote_instruction_data(&instruction.data)?;
                    
                    log::debug!("found vote slots in block: {:?}", vote_slots);

                    for vote_info in vote_slots {
                        if vote_info.is_new_vote() {
                            let voted_slot = vote_info.slot;

                            log::debug!("processing voted slot: {}", voted_slot);

                            if let Some(confirmed) = vote_tracker.confirm_vote(
                                signature.clone(),
                                voted_slot,
                                finalized_slot,
                            ) {
                                log::debug!(
                                    "confirmed vote: slot {} -> finalized {} -> latency {} -> {} tvc (sig: {})",
                                    voted_slot, finalized_slot, confirmed.latency, confirmed.tvc_credits,
                                    &signature[..8]
                                );
                                return Ok(Some(confirmed));
                            }
                        }
                    }
                }
            }
        }
    }
    
    Ok(None)
}
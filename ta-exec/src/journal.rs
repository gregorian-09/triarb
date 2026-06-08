use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::fs::{File, OpenOptions};

use ta_core::OpportunityId;

/// A single entry in the append-only order journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JournalEntry {
    Intent {
        opp_id: String,
        ts_ns: u64,
        venue: String,
        symbol: String,
        side: String,
    },
    Fill {
        opp_id: String,
        ts_ns: u64,
        leg: usize,
        venue: String,
        symbol: String,
        fill_price: i64,
        fill_size: i64,
    },
    Fail {
        opp_id: String,
        ts_ns: u64,
        leg: usize,
        reason: String,
    },
    Hedge {
        opp_id: String,
        ts_ns: u64,
        leg: usize,
        venue: String,
        symbol: String,
        hedge_price: i64,
    },
}

/// Append-only JSONL journal for crash recovery.
///
/// Every order intent is written to disk *before* submission.
/// On restart, `find_unacknowledged()` reveals orders that were
/// sent but never confirmed — preventing double-spend.
pub struct FileOrderLog {
    #[allow(dead_code)]
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    entries: Vec<JournalEntry>,
}

impl FileOrderLog {
    /// Opens or creates the journal at `path`.
    /// Reads existing entries into memory for replay.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .append(true)
            .open(&path)?;

        let entries = Self::read_all(&file)?;
        let writer = BufWriter::new(file);

        Ok(Self {
            path,
            writer: Some(writer),
            entries,
        })
    }

    fn read_all(file: &File) -> std::io::Result<Vec<JournalEntry>> {
        let reader = BufReader::new(file);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<JournalEntry>(&line) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    tracing::warn!("journal: skipping malformed entry: {e}");
                }
            }
        }
        Ok(entries)
    }

    /// Record an order intent *before* submission.
    pub fn record_intent(&mut self, opp_id: &OpportunityId, venue: &str, symbol: &str, side: &str) {
        let entry = JournalEntry::Intent {
            opp_id: format!("{opp_id:?}"),
            ts_ns: nanos(),
            venue: venue.to_string(),
            symbol: symbol.to_string(),
            side: side.to_string(),
        };
        self.append(&entry);
    }

    /// Record a successful fill.
    pub fn record_fill(
        &mut self,
        opp_id: &OpportunityId,
        leg: usize,
        venue: &str,
        symbol: &str,
        fill_price: i64,
        fill_size: i64,
    ) {
        let entry = JournalEntry::Fill {
            opp_id: format!("{opp_id:?}"),
            ts_ns: nanos(),
            leg,
            venue: venue.to_string(),
            symbol: symbol.to_string(),
            fill_price,
            fill_size,
        };
        self.append(&entry);
    }

    /// Record a submission failure.
    pub fn record_fail(&mut self, opp_id: &OpportunityId, leg: usize, reason: &str) {
        let entry = JournalEntry::Fail {
            opp_id: format!("{opp_id:?}"),
            ts_ns: nanos(),
            leg,
            reason: reason.to_string(),
        };
        self.append(&entry);
    }

    /// Record a hedge action.
    pub fn record_hedge(
        &mut self,
        opp_id: &OpportunityId,
        leg: usize,
        venue: &str,
        symbol: &str,
        hedge_price: i64,
    ) {
        let entry = JournalEntry::Hedge {
            opp_id: format!("{opp_id:?}"),
            ts_ns: nanos(),
            leg,
            venue: venue.to_string(),
            symbol: symbol.to_string(),
            hedge_price,
        };
        self.append(&entry);
    }

    /// Returns all entries whose intent has no corresponding Fill or Fail.
    pub fn find_unacknowledged(&self) -> Vec<&JournalEntry> {
        let mut unacked = Vec::new();
        for entry in &self.entries {
            if let JournalEntry::Intent { opp_id, .. } = entry {
                let has_confirmation = self.entries.iter().any(|e| match e {
                    JournalEntry::Fill { opp_id: id, .. }
                    | JournalEntry::Fail { opp_id: id, .. } => id == opp_id,
                    _ => false,
                });
                if !has_confirmation {
                    unacked.push(entry);
                }
            }
        }
        unacked
    }

    /// All entries in order.
    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// Log a warning for every unacknowledged intent (called on startup).
    pub fn report_unacknowledged(&self) {
        let unacked = self.find_unacknowledged();
        if unacked.is_empty() {
            tracing::info!("journal: all orders acknowledged, no recovery needed");
        } else {
            tracing::warn!(
                "journal: {} unacknowledged order(s) found — may need manual reconciliation",
                unacked.len()
            );
            for entry in unacked {
                tracing::warn!("journal: unacknowledged: {entry:?}");
            }
        }
    }

    fn append(&mut self, entry: &JournalEntry) {
        let line = serde_json::to_string(entry).expect("serialize journal entry");
        if let Some(ref mut writer) = self.writer {
            let _ = writeln!(writer, "{line}");
            let _ = writer.flush();
        }
        self.entries.push(entry.clone());
    }
}

fn nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("ta_journal_{name}_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn test_journal_create_replay() {
        let path = test_path("create_replay");
        let mut log = FileOrderLog::open(&path).unwrap();
        assert!(log.entries().is_empty());

        // simulate opp id
        let id = OpportunityId::from_opportunity(&ta_core::ArbitrageOpportunity {
            triangle: ta_core::Triangle {
                leg_a: ("A".into(), "B".into()),
                leg_b: ("B".into(), "C".into()),
                leg_c: ("C".into(), "A".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        });

        log.record_intent(&id, "BINANCE", "BTCUSDT", "Buy");
        log.record_fill(&id, 0, "BINANCE", "BTCUSDT", 50000, 100);
        drop(log); // close file

        // reopen and verify
        let log2 = FileOrderLog::open(&path).unwrap();
        assert_eq!(log2.entries().len(), 2);
        assert!(log2.find_unacknowledged().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_unacknowledged_detected() {
        let path = test_path("unacknowledged");
        let mut log = FileOrderLog::open(&path).unwrap();

        let id1 = OpportunityId::from_opportunity(&ta_core::ArbitrageOpportunity {
            triangle: ta_core::Triangle {
                leg_a: ("A".into(), "B".into()),
                leg_b: ("B".into(), "C".into()),
                leg_c: ("C".into(), "A".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        });
        let id2 = OpportunityId::from_opportunity(&ta_core::ArbitrageOpportunity {
            triangle: ta_core::Triangle {
                leg_a: ("X".into(), "Y".into()),
                leg_b: ("Y".into(), "Z".into()),
                leg_c: ("Z".into(), "X".into()),
                opportunity_bps: 10.0,
            },
            routes: Vec::new(),
            expected_profit_bps: 5.0,
            ts_ns: 0,
        });

        log.record_intent(&id1, "BINANCE", "BTCUSDT", "Buy");
        log.record_fill(&id1, 0, "BINANCE", "BTCUSDT", 50000, 100);
        log.record_intent(&id2, "BINANCE", "ETHBTC", "Sell"); // no fill/fail

        let unacked = log.find_unacknowledged();
        assert_eq!(unacked.len(), 1);
        assert!(matches!(unacked[0], JournalEntry::Intent { .. }));

        let _ = std::fs::remove_file(&path);
    }
}

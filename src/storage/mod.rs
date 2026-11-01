use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;

const SCHEMA_VERSION: i32 = 6;

pub struct Storage {
    conn: Connection,
}

// === Row types ===

#[derive(Debug, Clone)]
pub struct QsoRecord {
    pub guid: String,
    pub starttime: DateTime<Utc>,
    pub endtime: Option<DateTime<Utc>>,
    pub frequency: i64,
    pub band: String,
    pub bandwidth: String,
    pub callsign: String,
    pub my_callsign: String,
    pub snr_received: Option<i32>,
    pub snr_sent: Option<i32>,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub comments: Option<String>,
}

/// The qso columns beyond QsoRecord (as in VarAC).
#[derive(Debug, Clone, Default)]
pub struct QsoExtra {
    pub locator: String,
    pub my_power: String,
    pub my_rig: String,
    pub my_antenna: String,
    pub digipeater: String,
    pub mode: String,
    pub submode: String,
    pub me_away: bool,
    pub is_ping: bool,
    pub version: String,
    pub slot: String,
}

pub struct QsoStats {
    pub total: i64,
    pub unique_calls: i64,
    pub unique_grids: i64,
    pub band_counts: Vec<(String, i64)>,
}

#[derive(Debug, Clone)]
pub struct ChatRecord {
    pub guid: String,
    pub entry_type: i32, // 1=incoming, 2=outgoing, 3=system
    pub qso_guid: String,
    pub callsign: String,
    pub entry: String,
    pub creation_time: DateTime<Utc>,
    pub chat_id: Option<i64>,
    pub reply_on_chat_id: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct VMailRecord {
    pub guid: String,
    pub creation_time: DateTime<Utc>,
    pub sent_time: Option<DateTime<Utc>>,
    pub received_time: Option<DateTime<Utc>>,
    pub folder_id: i32,
    pub vmail_to: String,
    pub vmail_from: String,
    pub vmail_via: Option<String>,
    pub subject: String,
    pub msg: String,
    pub read_status: bool,
    pub is_deleted: bool,
    pub urgent: bool,
    pub delivery_band: Option<String>,
    pub delivery_snr: Option<i32>,
    pub frequency: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VMailFolder {
    Inbox = 1,
    Sent = 2,
    Outbox = 3,
    #[allow(dead_code)]
    Parking = 4,
    /// VarAC's Drafts (folder 5): composed, not queued
    Drafts = 5,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BroadcastRecord {
    pub guid: String,
    pub broadcast_time: DateTime<Utc>,
    pub frequency: i64,
    pub from_callsign: String,
    pub to_callsign: String,
    pub via_callsign: Option<String>,
    pub broadcast_message: String,
    pub snr: Option<i32>,
    pub band: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AlertRecord {
    pub guid: String,
    pub source_id: i64,
    pub from_callsign: String,
    pub to_callsign: String,
    pub alert_tag: String,
    pub alert_name: String,
    pub source_text: String,
    pub alert_time: DateTime<Utc>,
    pub read: bool,
    pub folder_id: i64,
    pub frequency: i64,
}

#[derive(Debug, Clone)]
pub struct ContactRecord {
    pub callsign: String,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub locator: Option<String>,
    pub notes: Option<String>,
    #[allow(dead_code)]
    pub last_qso: Option<DateTime<Utc>>,
    pub qso_count: i64,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct CqFrameRecord {
    pub guid: String,
    pub cqframe_time: DateTime<Utc>,
    pub cqframe_type_id: i32, // 1=CQ, 2=Beacon
    pub frequency: i64,
    pub bandwidth: String,
    pub from_callsign: String,
    pub snr: Option<i32>,
    pub slot: Option<i32>,
    pub locator: Option<String>,
    pub band: Option<String>,
}

#[allow(dead_code)]
impl Storage {
    pub fn open(path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        let mut storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    /// A second connection to the same database file (for the background protocol task).
    /// None for in-memory databases.
    pub fn reopen(&self) -> Option<Self> {
        let path = self.conn.path().filter(|p| !p.is_empty())?.to_string();
        Self::open(Path::new(&path)).ok()
    }

    pub fn open_in_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        let mut storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&mut self) -> SqlResult<()> {
        let version: i32 = self.conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);

        // The schema is all CREATE … IF NOT EXISTS / INSERT OR IGNORE, so apply it on every
        // open: a table added without bumping SCHEMA_VERSION (vmail_attachment was) would
        // otherwise never reach existing databases.
        self.conn.execute_batch(SCHEMA_SQL)?;
        // Columns added to existing tables after release (CREATE … IF NOT EXISTS won't)
        for (table, column, decl) in [
            ("vmail", "has_attachment", "INTEGER DEFAULT 0"),
            ("vmail_relay_notification", "urgent", "INTEGER DEFAULT 0"),
            // (the rest of VarAC's qso row)
            ("qso", "locator", "TEXT"),
            ("qso", "my_power", "TEXT"),
            ("qso", "my_rig", "TEXT"),
            ("qso", "my_antenna", "TEXT"),
            ("qso", "digipeater", "TEXT"),
            ("qso", "mode", "TEXT"),
            ("qso", "submode", "TEXT"),
            ("qso", "me_away_during_qso", "INTEGER DEFAULT 0"),
        ] {
            let has: bool = self.conn
                .prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"))?
                .exists(params![column])?;
            if !has {
                self.conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
            }
        }
        if version < SCHEMA_VERSION {
            self.conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        Ok(())
    }

    // === QSO ===

    pub fn insert_qso(&self, qso: &QsoRecord) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO qso (guid, starttime, endtime, frequency, band, bandwidth,
             callsign, my_callsign, snr_received, snr_sent, name, qth, comments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                qso.guid,
                fmt_dt(&qso.starttime),
                qso.endtime.as_ref().map(fmt_dt),
                qso.frequency,
                qso.band,
                qso.bandwidth,
                qso.callsign,
                qso.my_callsign,
                qso.snr_received,
                qso.snr_sent,
                qso.name,
                qso.qth,
                qso.comments,
            ],
        )?;
        Ok(())
    }

    /// VarAC's qso row as written at the end of a link: the QSO log
    /// fields plus the station details, pings flagged.
    pub fn insert_qso_full(&self, qso: &QsoRecord, x: &QsoExtra) -> SqlResult<()> {
        self.insert_qso(qso)?;
        self.conn.execute(
            "UPDATE qso SET locator = ?2, my_power = ?3, my_rig = ?4, my_antenna = ?5, digipeater = ?6,
             mode = ?7, submode = ?8, me_away_during_qso = ?9, is_ping = ?10, varac_version = ?11, slot = ?12
             WHERE guid = ?1",
            params![qso.guid, x.locator, x.my_power, x.my_rig, x.my_antenna, x.digipeater, x.mode, x.submode,
                    x.me_away as i32, x.is_ping as i32, x.version, x.slot],
        )?;
        Ok(())
    }

    /// The LOG button after the link ended: the edited QSO log fields go into its row.
    #[allow(clippy::too_many_arguments)]
    pub fn update_qso_from_log(&self, guid: &str, callsign: &str, band: &str, name: &str, qth: &str, locator: &str,
                               power: &str, snr_sent: Option<i32>, snr_received: Option<i32>, end: Option<&DateTime<Utc>>) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE qso SET callsign = ?2, band = ?3, name = ?4, qth = ?5, locator = ?6, my_power = ?7,
             snr_sent = ?8, snr_received = ?9, endtime = COALESCE(?10, endtime) WHERE guid = ?1",
            params![guid, callsign, band, name, qth, locator, power, snr_sent, snr_received, end.map(fmt_dt)],
        )?;
        Ok(())
    }

    pub fn update_qso_end(&self, guid: &str, endtime: &DateTime<Utc>) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE qso SET endtime = ?1 WHERE guid = ?2",
            params![fmt_dt(endtime), guid],
        )?;
        Ok(())
    }

    pub fn update_qso_comments(&self, guid: &str, comments: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE qso SET comments = ?1 WHERE guid = ?2",
            params![comments, guid],
        )?;
        Ok(())
    }

    pub fn delete_qso(&self, guid: &str) -> SqlResult<()> {
        self.conn.execute("DELETE FROM datastream WHERE qso_guid = ?1", params![guid])?;
        self.conn.execute("DELETE FROM qso WHERE guid = ?1", params![guid])?;
        Ok(())
    }

    pub fn heard_stations(&self, limit: usize) -> SqlResult<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT callsign, last_heard, total_count FROM (
                 SELECT callsign, MAX(starttime) as last_heard, COUNT(*) as total_count
                 FROM qso WHERE COALESCE(is_ping, 0) = 0 GROUP BY callsign
                 UNION ALL
                 SELECT from_callsign as callsign, MAX(cqframe_time) as last_heard, COUNT(*) as total_count
                 FROM cqframe GROUP BY from_callsign
             ) GROUP BY callsign
             ORDER BY MAX(last_heard) DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect()
    }

    pub fn upsert_heard_station(&self, _callsign: &str) -> SqlResult<()> {
        Ok(())
    }

    pub fn update_qso_details(
        &self,
        guid: &str,
        name: Option<&str>,
        qth: Option<&str>,
        snr_received: Option<i32>,
        snr_sent: Option<i32>,
    ) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE qso SET name = COALESCE(?1, name), qth = COALESCE(?2, qth),
             snr_received = COALESCE(?3, snr_received), snr_sent = COALESCE(?4, snr_sent)
             WHERE guid = ?5",
            params![name, qth, snr_received, snr_sent, guid],
        )?;
        Ok(())
    }

    pub fn get_qso(&self, guid: &str) -> SqlResult<Option<QsoRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, starttime, endtime, frequency, band, bandwidth,
                    callsign, my_callsign, snr_received, snr_sent, name, qth, comments
             FROM qso WHERE guid = ?1"
        )?;
        let mut rows = stmt.query_map(params![guid], |row| {
            Ok(QsoRecord {
                guid: row.get(0)?,
                starttime: parse_dt(&row.get::<_, String>(1)?),
                endtime: row.get::<_, Option<String>>(2)?.map(|s| parse_dt(&s)),
                frequency: row.get(3)?,
                band: row.get(4)?,
                bandwidth: row.get(5)?,
                callsign: row.get(6)?,
                my_callsign: row.get(7)?,
                snr_received: row.get(8)?,
                snr_sent: row.get(9)?,
                name: row.get(10)?,
                qth: row.get(11)?,
                comments: row.get(12)?,
            })
        })?;
        rows.next().transpose()
    }

    pub fn recent_qsos(&self, limit: usize) -> SqlResult<Vec<QsoRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, starttime, endtime, frequency, band, bandwidth,
                    callsign, my_callsign, snr_received, snr_sent, name, qth, comments
             FROM qso WHERE COALESCE(is_ping, 0) = 0 ORDER BY starttime DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(QsoRecord {
                guid: row.get(0)?,
                starttime: parse_dt(&row.get::<_, String>(1)?),
                endtime: row.get::<_, Option<String>>(2)?.map(|s| parse_dt(&s)),
                frequency: row.get(3)?,
                band: row.get(4)?,
                bandwidth: row.get(5)?,
                callsign: row.get(6)?,
                my_callsign: row.get(7)?,
                snr_received: row.get(8)?,
                snr_sent: row.get(9)?,
                name: row.get(10)?,
                qth: row.get(11)?,
                comments: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// QSOs with `call` (any SSID), newest first (VarAC "Previous QSOs").
    pub fn qsos_with(&self, call: &str) -> SqlResult<Vec<QsoRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, starttime, endtime, frequency, band, bandwidth,
                    callsign, my_callsign, snr_received, snr_sent, name, qth, comments
             FROM qso WHERE COALESCE(is_ping, 0) = 0 AND (upper(callsign) = upper(?1) OR upper(callsign) LIKE upper(?1) || '-%')
             ORDER BY starttime DESC LIMIT 500"
        )?;
        let rows = stmt.query_map(params![call], |row| {
            Ok(QsoRecord {
                guid: row.get(0)?,
                starttime: parse_dt(&row.get::<_, String>(1)?),
                endtime: row.get::<_, Option<String>>(2)?.map(|s| parse_dt(&s)),
                frequency: row.get(3)?,
                band: row.get(4)?,
                bandwidth: row.get(5)?,
                callsign: row.get(6)?,
                my_callsign: row.get(7)?,
                snr_received: row.get(8)?,
                snr_sent: row.get(9)?,
                name: row.get(10)?,
                qth: row.get(11)?,
                comments: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Beacons and CQs heard from `call` (any SSID) since `since`, newest first.
    pub fn cqframes_from(&self, call: &str, since: DateTime<Utc>) -> SqlResult<Vec<CqFrameRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, cqframe_time, cqframe_type_id, frequency, bandwidth,
                    from_callsign, snr, slot, locator, band
             FROM cqframe WHERE upper(from_callsign) = upper(?1) OR upper(from_callsign) LIKE upper(?1) || '-%'
             ORDER BY cqframe_time DESC LIMIT 2000"
        )?;
        let rows = stmt.query_map(params![call], |row| {
            Ok(CqFrameRecord {
                guid: row.get(0)?,
                cqframe_time: parse_dt(&row.get::<_, String>(1)?),
                cqframe_type_id: row.get(2)?,
                frequency: row.get(3)?,
                bandwidth: row.get(4)?,
                from_callsign: row.get(5)?,
                snr: row.get(6)?,
                slot: row.get(7)?,
                locator: row.get(8)?,
                band: row.get(9)?,
            })
        })?;
        Ok(rows.filter_map(Result::ok).filter(|r| r.cqframe_time >= since).collect())
    }

    /// Broadcasts sent by `call` (any SSID), newest first.
    pub fn broadcasts_from(&self, call: &str) -> SqlResult<Vec<BroadcastRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, broadcast_time, frequency, from_callsign, to_callsign,
                    via_callsign, broadcast_message, snr, band
             FROM broadcast WHERE upper(from_callsign) = upper(?1) OR upper(from_callsign) LIKE upper(?1) || '-%'
             ORDER BY broadcast_time DESC LIMIT 1000"
        )?;
        let rows = stmt.query_map(params![call], |row| {
            Ok(BroadcastRecord {
                guid: row.get(0)?,
                broadcast_time: parse_dt(&row.get::<_, String>(1)?),
                frequency: row.get(2)?,
                from_callsign: row.get(3)?,
                to_callsign: row.get(4)?,
                via_callsign: row.get(5)?,
                broadcast_message: row.get(6)?,
                snr: row.get(7)?,
                band: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    /// VarAC's ALE candidates for one pass: the frequencies `call` was heard on since
    /// `since` (only in these UTC hours when given), best average SNR first.
    pub fn ale_pass(&self, call: &str, since: DateTime<Utc>, hours: Option<[u32; 3]>, exclude: u64) -> SqlResult<Vec<(u64, f32, u32, f64)>> {
        let mut sql = String::from(
            "SELECT frequency, AVG(snr) AS avg_snr, COUNT(id) AS beacons, MAX(cqframe_time) AS last_heard FROM cqframe
             WHERE (upper(from_callsign) = upper(?1) OR upper(from_callsign) LIKE upper(?1) || '-%')
               AND snr IS NOT NULL AND snr >= -100 AND snr <= 100 AND cqframe_time >= ?2 AND frequency != ?3",
        );
        if let Some([a, b, c]) = hours {
            sql.push_str(&format!(" AND CAST(strftime('%H', cqframe_time) AS INTEGER) IN ({a}, {b}, {c})"));
        }
        sql.push_str(" GROUP BY frequency ORDER BY avg_snr DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let now = Utc::now();
        let rows = stmt.query_map(params![call, fmt_dt(&since), exclude as i64], |r| {
            let last: String = r.get(3)?;
            let mins = (now - parse_dt(&last)).num_seconds() as f64 / 60.0;
            Ok((r.get::<_, i64>(0)?.max(0) as u64, r.get::<_, f64>(1)? as f32, r.get::<_, i64>(2)? as u32, mins))
        })?;
        rows.collect()
    }

    // === Files sent and received (VarAC's gallery: datastream rows with a file_path) ===

    pub fn log_file(&self, path: &std::path::Path, callsign: &str, outgoing: bool) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO file_log (file_path, callsign, creation_time, outgoing) VALUES (?1, ?2, ?3, ?4)",
            params![path.display().to_string(), callsign, fmt_dt(&Utc::now()), outgoing],
        )?;
        Ok(())
    }

    /// (path, callsign, time, outgoing), newest first.
    pub fn logged_files(&self) -> SqlResult<Vec<(String, String, DateTime<Utc>, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_path, callsign, creation_time, outgoing FROM file_log ORDER BY creation_time DESC LIMIT 500"
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default(), parse_dt(&r.get::<_, String>(2)?), r.get::<_, i64>(3)? != 0)))?;
        rows.collect()
    }

    // === SNR over a QSO (as in VarAC: direction 1 = partner, 2 = us) ===

    pub fn save_snr_series(&self, qso_guid: &str, his: &[(DateTime<Utc>, f32)], mine: &[(DateTime<Utc>, f32)]) -> SqlResult<()> {
        for (dir, series) in [(1, his), (2, mine)] {
            for (t, v) in series {
                self.conn.execute(
                    "INSERT INTO qso_snr_report (qso_guid, snr_direction, snr, creation_time) VALUES (?1, ?2, ?3, ?4)",
                    params![qso_guid, dir, v.round() as i64, fmt_dt(t)],
                )?;
            }
        }
        Ok(())
    }

    /// (partner series, our series) for a QSO.
    pub fn snr_series(&self, qso_guid: &str) -> SqlResult<(Vec<(DateTime<Utc>, f32)>, Vec<(DateTime<Utc>, f32)>)> {
        let mut stmt = self.conn.prepare(
            "SELECT snr_direction, snr, creation_time FROM qso_snr_report WHERE qso_guid = ?1 ORDER BY creation_time ASC"
        )?;
        let (mut his, mut mine) = (Vec::new(), Vec::new());
        for r in stmt.query_map(params![qso_guid], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?)))? {
            let (d, v, t) = r?;
            if d == 1 { &mut his } else { &mut mine }.push((parse_dt(&t), v as f32));
        }
        Ok((his, mine))
    }

    /// Stations with a locator heard or worked in the last `minutes`, for the map:
    /// (callsign, locator, kind) with kind 1 beacon, 2 CQ, 3 broadcast, 4 QSO.
    pub fn map_stations(&self, minutes: i64) -> SqlResult<Vec<(String, String, u8)>> {
        let since = fmt_dt(&(Utc::now() - chrono::Duration::minutes(minutes)));
        let mut out = Vec::new();
        let mut q = |sql: &str, kind_col: bool| -> SqlResult<()> {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, if kind_col { r.get::<_, i64>(2)? } else { 0 }))
            })?;
            for r in rows.flatten() {
                out.push(r);
            }
            Ok(())
        };
        q("SELECT c.from_callsign, COALESCE(NULLIF(c.locator, ''), k.locator), c.cqframe_type_id FROM cqframe c
           LEFT JOIN contacts k ON upper(k.callsign) = upper(c.from_callsign) WHERE c.cqframe_time > ?1", true)?;
        let mut kinds: Vec<(String, Option<String>, i64)> = out.drain(..).map(|(c, l, k)| (c, l, if k == 2 { 1 } else { 2 })).collect();
        let mut q2 = |sql: &str, kind: i64| -> SqlResult<()> {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
            for (c, l) in rows.flatten() {
                kinds.push((c, l, kind));
            }
            Ok(())
        };
        q2("SELECT b.from_callsign, k.locator FROM broadcast b LEFT JOIN contacts k ON upper(k.callsign) = upper(b.from_callsign) WHERE b.broadcast_time > ?1", 3)?;
        q2("SELECT q.callsign, k.locator FROM qso q LEFT JOIN contacts k ON upper(k.callsign) = upper(q.callsign) WHERE q.starttime > ?1 AND COALESCE(q.is_ping, 0) = 0", 4)?;
        Ok(kinds.into_iter().filter_map(|(c, l, k)| {
            let l = l.filter(|l| l.len() >= 4)?;
            Some((c.split('-').next().unwrap_or(&c).to_uppercase(), l.to_uppercase(), k as u8))
        }).collect())
    }

    // === Alerts (VarAC alert table; folder 1 Incoming, 2 Archived, 3 Sent) ===

    pub fn insert_alert(&self, a: &AlertRecord) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO alert (guid, source_id, from_callsign, to_callsign, alert_tag, alert_name, source_text,
                                alert_time, read_status, folder_id, frequency, is_deleted)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)",
            params![a.guid, a.source_id, a.from_callsign, a.to_callsign, a.alert_tag, a.alert_name, a.source_text,
                    fmt_dt(&a.alert_time), a.read, a.folder_id, a.frequency],
        )?;
        Ok(())
    }

    pub fn alerts(&self, folder_id: i64) -> SqlResult<Vec<AlertRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, source_id, from_callsign, to_callsign, alert_tag, alert_name, source_text,
                    alert_time, read_status, folder_id, frequency
             FROM alert WHERE folder_id = ?1 AND is_deleted = 0 ORDER BY alert_time DESC LIMIT 2000"
        )?;
        let rows = stmt.query_map(params![folder_id], |row| {
            Ok(AlertRecord {
                guid: row.get(0)?,
                source_id: row.get(1)?,
                from_callsign: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                to_callsign: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                alert_tag: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                alert_name: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                source_text: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                alert_time: parse_dt(&row.get::<_, String>(7)?),
                read: row.get::<_, i64>(8)? != 0,
                folder_id: row.get(9)?,
                frequency: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
            })
        })?;
        rows.collect()
    }

    pub fn unread_alert_count(&self) -> SqlResult<i64> {
        self.conn.query_row("SELECT count(*) FROM alert WHERE is_deleted = 0 AND read_status = 0 AND folder_id = 1", [], |r| r.get(0))
    }

    /// `guid` None = every alert of `folder_id`.
    pub fn mark_alert_read(&self, guid: Option<&str>, folder_id: i64) -> SqlResult<()> {
        match guid {
            Some(g) => self.conn.execute("UPDATE alert SET read_status = 1 WHERE guid = ?1", params![g])?,
            None => self.conn.execute("UPDATE alert SET read_status = 1 WHERE folder_id = ?1", params![folder_id])?,
        };
        Ok(())
    }

    pub fn move_alert(&self, guid: Option<&str>, from_folder: i64, to_folder: i64) -> SqlResult<()> {
        match guid {
            Some(g) => self.conn.execute("UPDATE alert SET folder_id = ?1 WHERE guid = ?2", params![to_folder, g])?,
            None => self.conn.execute("UPDATE alert SET folder_id = ?1 WHERE folder_id = ?2 AND is_deleted = 0", params![to_folder, from_folder])?,
        };
        Ok(())
    }

    pub fn delete_alert(&self, guid: Option<&str>, folder_id: i64) -> SqlResult<()> {
        match guid {
            Some(g) => self.conn.execute("UPDATE alert SET is_deleted = 1 WHERE guid = ?1", params![g])?,
            None => self.conn.execute("UPDATE alert SET is_deleted = 1 WHERE folder_id = ?1", params![folder_id])?,
        };
        Ok(())
    }

    pub fn qso_statistics(&self) -> SqlResult<QsoStats> {
        let total: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM qso WHERE COALESCE(is_ping, 0) = 0", [], |row| row.get(0))?;
        let unique_calls: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT callsign) FROM qso WHERE COALESCE(is_ping, 0) = 0", [], |row| row.get(0))?;
        let unique_grids: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT UPPER(SUBSTR(c.locator, 1, 4)))
             FROM contacts c
             JOIN qso q ON UPPER(q.callsign) = c.callsign
             WHERE c.locator IS NOT NULL AND LENGTH(c.locator) >= 4",
            [], |row| row.get(0))?;
        let mut band_counts = Vec::new();
        let mut stmt = self.conn.prepare(
            "SELECT COALESCE(band, '?'), COUNT(*) FROM qso WHERE COALESCE(is_ping, 0) = 0 GROUP BY band ORDER BY COUNT(*) DESC")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            if let Ok((band, count)) = row {
                band_counts.push((band, count));
            }
        }
        Ok(QsoStats { total, unique_calls, unique_grids, band_counts })
    }

    // === Chat / Datastream ===

    pub fn insert_chat(&self, rec: &ChatRecord) -> SqlResult<i64> {
        self.conn.execute(
            "INSERT INTO datastream (guid, datastream_entry_type_id, qso_guid,
             callsign, entry, creation_time, chat_id, reply_on_chat_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                rec.guid,
                rec.entry_type,
                rec.qso_guid,
                rec.callsign,
                rec.entry,
                fmt_dt(&rec.creation_time),
                rec.chat_id,
                rec.reply_on_chat_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn chat_history(&self, qso_guid: &str, limit: usize) -> SqlResult<Vec<ChatRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, datastream_entry_type_id, qso_guid, callsign, entry,
                    creation_time, chat_id, reply_on_chat_id
             FROM datastream WHERE qso_guid = ?1
             ORDER BY creation_time DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![qso_guid, limit as i64], |row| {
            Ok(ChatRecord {
                guid: row.get(0)?,
                entry_type: row.get(1)?,
                qso_guid: row.get(2)?,
                callsign: row.get(3)?,
                entry: row.get(4)?,
                creation_time: parse_dt(&row.get::<_, String>(5)?),
                chat_id: row.get(6)?,
                reply_on_chat_id: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    pub fn chat_history_with_callsign(&self, callsign: &str, limit: usize) -> SqlResult<Vec<ChatRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT d.guid, d.datastream_entry_type_id, d.qso_guid, d.callsign, d.entry,
                    d.creation_time, d.chat_id, d.reply_on_chat_id
             FROM datastream d
             JOIN qso q ON d.qso_guid = q.guid
             WHERE q.callsign = ?1
             ORDER BY d.creation_time DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![callsign, limit as i64], |row| {
            Ok(ChatRecord {
                guid: row.get(0)?,
                entry_type: row.get(1)?,
                qso_guid: row.get(2)?,
                callsign: row.get(3)?,
                entry: row.get(4)?,
                creation_time: parse_dt(&row.get::<_, String>(5)?),
                chat_id: row.get(6)?,
                reply_on_chat_id: row.get(7)?,
            })
        })?;
        let mut result: Vec<_> = rows.collect::<SqlResult<Vec<_>>>()?;
        result.reverse();
        Ok(result)
    }

    // === VMail ===

    pub fn insert_vmail(&self, vm: &VMailRecord) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO vmail (guid, creation_time, sent_time, received_time,
             folder_id, vmail_to, vmail_from, vmail_via, subject, msg,
             read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                vm.guid,
                fmt_dt(&vm.creation_time),
                vm.sent_time.as_ref().map(fmt_dt),
                vm.received_time.as_ref().map(fmt_dt),
                vm.folder_id,
                vm.vmail_to,
                vm.vmail_from,
                vm.vmail_via,
                vm.subject,
                vm.msg,
                vm.read_status,
                vm.is_deleted,
                vm.urgent,
                vm.delivery_band,
                vm.delivery_snr,
                vm.frequency,
            ],
        )?;
        Ok(())
    }

    pub fn get_vmail(&self, guid: &str) -> SqlResult<Option<VMailRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE guid = ?1"
        )?;
        let mut rows = stmt.query_map(params![guid], row_to_vmail)?;
        rows.next().transpose()
    }

    /// VMails `from` parked here in the last 30 days, still parked or already relayed —
    /// the data behind VarAC's <VRP> answer (newest first, at most 10).
    pub fn parked_vmails_from(&self, from: &str) -> SqlResult<Vec<VMailRecord>> {
        let since = Utc::now() - chrono::Duration::days(30);
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE upper(vmail_from) = upper(?1) AND folder_id IN (?2, ?3)
               AND is_deleted = 0 AND creation_time > ?4
             ORDER BY creation_time DESC LIMIT 10"
        )?;
        let rows = stmt.query_map(
            params![from, VMailFolder::Sent as i32, VMailFolder::Parking as i32, fmt_dt(&since)],
            row_to_vmail,
        )?;
        // Our own sent mail isn't "parked here": only mail addressed to someone else
        Ok(rows.collect::<SqlResult<Vec<_>>>()?.into_iter().filter(|v| !v.vmail_to.contains('@')).collect())
    }

    pub fn vmails_in_folder(&self, folder: VMailFolder, limit: usize) -> SqlResult<Vec<VMailRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE folder_id = ?1 AND is_deleted = 0
             ORDER BY creation_time DESC LIMIT ?2"
        )?;
        let rows = stmt.query_map(params![folder as i32, limit as i64], row_to_vmail)?;
        rows.collect()
    }

    /// Remember a file attached to a VMail (stored by path).
    pub fn add_vmail_attachment(&self, vmail_guid: &str, path: &std::path::Path) -> SqlResult<()> {
        let size = std::fs::metadata(path).map(|m| m.len() as i64).ok();
        self.conn.execute(
            "INSERT INTO vmail_attachment (guid, vmail_guid, file_name, file_size_bytes, creation_time)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uuid::Uuid::new_v4().to_string(), vmail_guid, path.to_string_lossy(), size, fmt_dt(&Utc::now())],
        )?;
        self.conn.execute("UPDATE vmail SET has_attachment = 1 WHERE guid = ?1", params![vmail_guid])?;
        Ok(())
    }

    /// Remember an email VMail's REPLY TO address and raw flag.
    pub fn set_vmail_email(&self, vmail_guid: &str, reply_to: Option<&str>, raw: bool) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO vmail_email (vmail_guid, reply_to, raw) VALUES (?1, ?2, ?3)",
            params![vmail_guid, reply_to, raw as i32],
        )?;
        Ok(())
    }

    /// An email VMail's (REPLY TO, raw); (None, false) when none was stored.
    pub fn vmail_email(&self, vmail_guid: &str) -> SqlResult<(Option<String>, bool)> {
        let r = self.conn.query_row(
            "SELECT reply_to, raw FROM vmail_email WHERE vmail_guid = ?1",
            params![vmail_guid],
            |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, i32>(1)? != 0)),
        );
        match r {
            Ok(v) => Ok(v),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok((None, false)),
            Err(e) => Err(e),
        }
    }

    /// Email VMails we took as gateway and haven't mailed yet (Parking, To = an address),
    /// oldest first (VarAC's gateway queue).
    pub fn gateway_emails_pending(&self) -> SqlResult<Vec<VMailRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE folder_id = ?1 AND is_deleted = 0 AND vmail_to LIKE '%@%'
             ORDER BY creation_time ASC"
        )?;
        let rows = stmt.query_map(params![VMailFolder::Parking as i32], row_to_vmail)?;
        rows.collect()
    }

    /// Paths of the files attached to a VMail.
    pub fn vmail_attachments(&self, vmail_guid: &str) -> SqlResult<Vec<std::path::PathBuf>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_name FROM vmail_attachment WHERE vmail_guid = ?1 AND is_deleted = 0 ORDER BY id",
        )?;
        let rows = stmt.query_map(params![vmail_guid], |r| r.get::<_, String>(0))?;
        rows.map(|r| r.map(std::path::PathBuf::from)).collect()
    }

    pub fn outbox_vmails_for(&self, callsign: &str) -> SqlResult<Vec<VMailRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE folder_id = ?1 AND is_deleted = 0 AND UPPER(vmail_to) = UPPER(?2)
             ORDER BY creation_time ASC"
        )?;
        let rows = stmt.query_map(params![VMailFolder::Outbox as i32, callsign], row_to_vmail)?;
        rows.collect()
    }

    /// As in VarAC: Outbox and Parking mail for `call` (or
    /// its full callsign, or `CALL/…` / `…/CALL`), in the order it was stored.
    pub fn vmails_waiting_for(&self, call: &str, full: &str) -> SqlResult<Vec<VMailRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE folder_id IN (3, 4) AND is_deleted = 0
               AND (UPPER(vmail_to) IN (UPPER(?1), UPPER(?2)) OR UPPER(vmail_to) LIKE UPPER(?1) || '/%'
                    OR UPPER(vmail_to) LIKE '%/' || UPPER(?1))
             ORDER BY rowid ASC LIMIT 100"
        )?;
        let rows = stmt.query_map(params![call, full], row_to_vmail)?;
        rows.collect()
    }

    /// (as in VarAC: only the folder; sent_time stays what it was)
    pub fn move_vmail_to_sent(&self, guid: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE vmail SET folder_id = ?1 WHERE guid = ?2",
            params![VMailFolder::Sent as i32, guid],
        )?;
        Ok(())
    }

    pub fn set_vmail_via(&self, guid: &str, via: &str) -> SqlResult<()> {
        self.conn.execute("UPDATE vmail SET vmail_via = ?1 WHERE guid = ?2", params![via, guid])?;
        Ok(())
    }

    pub fn mark_vmail_read(&self, guid: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE vmail SET read_status = 1 WHERE guid = ?1",
            params![guid],
        )?;
        Ok(())
    }

    /// One VMail by its guid (any folder).
    pub fn vmail_by_guid(&self, guid: &str) -> SqlResult<Option<VMailRecord>> {
        let r = self.conn.query_row(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE guid = ?1",
            params![guid],
            row_to_vmail,
        );
        match r {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Remove a VMail for good (VarAC replaces an edited VMail with a new row).
    pub fn delete_vmail_row(&self, guid: &str) -> SqlResult<()> {
        self.conn.execute("DELETE FROM vmail_attachment WHERE vmail_guid = ?1", params![guid])?;
        self.conn.execute("DELETE FROM vmail_email WHERE vmail_guid = ?1", params![guid]).ok();
        self.conn.execute("DELETE FROM vmail WHERE guid = ?1", params![guid])?;
        Ok(())
    }

    /// A station told us (`<SMP>`/`<SMU>`) it holds VMail for us: one row per station,
    /// the latest (VarAC soft-deletes the earlier ones).
    pub fn record_relay_notification(&self, from: &str, frequency: i64, urgent: bool, at: &DateTime<Utc>) -> SqlResult<()> {
        let from = from.split('-').next().unwrap_or(from).to_uppercase();
        self.conn.execute("UPDATE vmail_relay_notification SET is_deleted = 1 WHERE upper(from_callsign) = ?1", params![from])?;
        self.conn.execute(
            "INSERT INTO vmail_relay_notification (guid, relay_notification_time, frequency, from_callsign, urgent)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![uuid::Uuid::new_v4().to_string(), fmt_dt(at), frequency, from, urgent],
        )?;
        Ok(())
    }

    /// The relay notifications still open: (time, frequency, callsign, urgent), newest first.
    pub fn relay_notifications(&self) -> SqlResult<Vec<(DateTime<Utc>, i64, String, bool)>> {
        let mut stmt = self.conn.prepare(
            "SELECT relay_notification_time, COALESCE(frequency, 0), from_callsign, COALESCE(urgent, 0)
             FROM vmail_relay_notification WHERE is_deleted = 0 ORDER BY relay_notification_time DESC",
        )?;
        let rows = stmt.query_map([], |r| Ok((parse_dt(&r.get::<_, String>(0)?), r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)))?;
        rows.collect()
    }

    /// Done with a station's notifications (collected its mail, or deleted them).
    pub fn clear_relay_notifications(&self, from: &str) -> SqlResult<()> {
        let from = from.split('-').next().unwrap_or(from).to_uppercase();
        self.conn.execute("UPDATE vmail_relay_notification SET is_deleted = 1 WHERE upper(from_callsign) = ?1", params![from])?;
        Ok(())
    }

    pub fn mark_vmail_deleted(&self, guid: &str) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE vmail SET is_deleted = 1 WHERE guid = ?1",
            params![guid],
        )?;
        Ok(())
    }

    pub fn unread_vmail_count(&self) -> SqlResult<i64> {
        self.conn.query_row(
            "SELECT COUNT(*) FROM vmail WHERE folder_id = 1 AND read_status = 0 AND is_deleted = 0",
            [],
            |r| r.get(0),
        )
    }

    pub fn pending_outbox_vmails(&self) -> SqlResult<Vec<VMailRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, creation_time, sent_time, received_time, folder_id,
                    vmail_to, vmail_from, vmail_via, subject, msg,
                    read_status, is_deleted, urgent, delivery_band, delivery_snr, frequency
             FROM vmail WHERE folder_id IN (3, 4) AND is_deleted = 0
             ORDER BY urgent DESC, creation_time ASC"
        )?;
        let rows = stmt.query_map([], row_to_vmail)?;
        rows.collect()
    }

    pub fn move_vmail_to_folder(&self, guid: &str, folder: VMailFolder) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE vmail SET folder_id = ?1 WHERE guid = ?2",
            params![folder as i32, guid],
        )?;
        Ok(())
    }

    pub fn mark_vmail_sent(&self, guid: &str, sent_time: &DateTime<Utc>) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE vmail SET folder_id = 2, sent_time = ?1 WHERE guid = ?2",
            params![fmt_dt(sent_time), guid],
        )?;
        Ok(())
    }

    // === Broadcast ===

    pub fn insert_broadcast(&self, bc: &BroadcastRecord) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO broadcast (guid, broadcast_time, frequency,
             from_callsign, to_callsign, via_callsign, broadcast_message, snr, band)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                bc.guid,
                fmt_dt(&bc.broadcast_time),
                bc.frequency,
                bc.from_callsign,
                bc.to_callsign,
                bc.via_callsign,
                bc.broadcast_message,
                bc.snr,
                bc.band,
            ],
        )?;
        Ok(())
    }

    pub fn recent_broadcasts(&self, limit: usize) -> SqlResult<Vec<BroadcastRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, broadcast_time, frequency, from_callsign, to_callsign,
                    via_callsign, broadcast_message, snr, band
             FROM broadcast ORDER BY broadcast_time DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(BroadcastRecord {
                guid: row.get(0)?,
                broadcast_time: parse_dt(&row.get::<_, String>(1)?),
                frequency: row.get(2)?,
                from_callsign: row.get(3)?,
                to_callsign: row.get(4)?,
                via_callsign: row.get(5)?,
                broadcast_message: row.get(6)?,
                snr: row.get(7)?,
                band: row.get(8)?,
            })
        })?;
        rows.collect()
    }

    pub fn is_duplicate_broadcast(
        &self,
        frequency: i64,
        from: &str,
        to: &str,
        via: Option<&str>,
        msg: &str,
        within_secs: i64,
    ) -> SqlResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM broadcast
             WHERE frequency = ?1 AND from_callsign = ?2 AND to_callsign = ?3
               AND COALESCE(via_callsign, '') = COALESCE(?4, '')
               AND broadcast_message = ?5
               AND broadcast_time > datetime('now', ?6)",
            params![
                frequency,
                from,
                to,
                via,
                msg,
                format!("-{within_secs} seconds"),
            ],
            |r| r.get(0),
        )?;
        Ok(count > 0)
    }

    // === CQ Frames ===

    pub fn insert_cqframe(&self, cq: &CqFrameRecord) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO cqframe (guid, cqframe_time, cqframe_type_id,
             frequency, bandwidth, from_callsign, snr, slot, locator, band)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                cq.guid,
                fmt_dt(&cq.cqframe_time),
                cq.cqframe_type_id,
                cq.frequency,
                cq.bandwidth,
                cq.from_callsign,
                cq.snr,
                cq.slot,
                cq.locator,
                cq.band,
            ],
        )?;
        Ok(())
    }

    /// VarAC's last-heard list: each station's latest CQ/beacon since `since`, on `band`
    /// (None = all bands), newest first.
    pub fn last_heard(&self, since: DateTime<Utc>, band: Option<&str>, limit: usize) -> SqlResult<Vec<crate::varac::messages::HeardEntry>> {
        let band_clause = if band.is_some() { " AND band = ?2" } else { "" };
        let sql = format!(
            "SELECT from_callsign, cqframe_type_id, cqframe_time, frequency, snr, COALESCE(band, '')
             FROM cqframe WHERE id IN (SELECT max(id) FROM cqframe WHERE cqframe_time > ?1{band_clause} GROUP BY upper(from_callsign))
             ORDER BY cqframe_time DESC LIMIT {limit}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let map = |row: &rusqlite::Row| -> rusqlite::Result<crate::varac::messages::HeardEntry> {
            Ok(crate::varac::messages::HeardEntry {
                callsign: row.get(0)?,
                source: if row.get::<_, i32>(1)? == 2 { "BEACON" } else { "CQ" }.into(),
                time: parse_dt(&row.get::<_, String>(2)?),
                frequency_hz: row.get(3)?,
                snr: row.get(4)?,
                band: row.get(5)?,
            })
        };
        let rows = match band {
            Some(b) => stmt.query_map(params![fmt_dt(&since), b], map)?.collect(),
            None => stmt.query_map(params![fmt_dt(&since)], map)?.collect(),
        };
        rows
    }

    /// When we last heard `call` (CQ/beacon or QSO), with frequency and SNR, as VarAC's
    /// `<LHC:>` answer needs; None if never (or without an SNR, which VarAC requires).
    /// EmComm `<LCR>` (VarAC): the last `limit` QSOs that weren't pings, newest first.
    /// Only finished ones: VarAC logs a QSO at its end, so the one in progress isn't listed.
    pub fn last_connections(&self, limit: u32) -> SqlResult<Vec<(String, DateTime<Utc>, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT callsign, starttime, frequency FROM qso WHERE COALESCE(is_ping, 0) = 0
             AND endtime IS NOT NULL ORDER BY starttime DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit], |r| Ok((r.get(0)?, parse_dt(&r.get::<_, String>(1)?), r.get(2)?)))?;
        rows.collect()
    }

    /// Path Finder (VarAC): the latest time `call` was heard since `since` — CQ/beacon,
    /// broadcast or QSO — as (time, SNR, frequency).
    pub fn path_finder_heard(&self, call: &str, since: &DateTime<Utc>) -> SqlResult<Option<(DateTime<Utc>, Option<i32>, i64)>> {
        let base = call.split(['-', '/']).next().unwrap_or(call).trim().to_uppercase();
        let since = fmt_dt(since);
        let row = self.conn.query_row(
            "SELECT t, snr, f FROM (
                SELECT cqframe_time AS t, snr, frequency AS f FROM cqframe
                 WHERE (upper(from_callsign) = ?1 OR upper(from_callsign) LIKE ?1 || '-%') AND cqframe_time >= ?2
                UNION ALL
                SELECT broadcast_time, snr, frequency FROM broadcast
                 WHERE (upper(from_callsign) = ?1 OR upper(from_callsign) LIKE ?1 || '-%') AND broadcast_time >= ?2
                UNION ALL
                SELECT starttime, snr_sent, frequency FROM qso
                 WHERE upper(callsign) = ?1 AND starttime >= ?2
             ) ORDER BY t DESC LIMIT 1",
            params![base, since],
            |r| Ok((parse_dt(&r.get::<_, String>(0)?), r.get::<_, Option<i32>>(1)?, r.get::<_, i64>(2)?)),
        );
        match row {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn last_heard_callsign(&self, call: &str) -> SqlResult<Option<crate::varac::messages::HeardEntry>> {
        let base = call.split(['-', '/']).next().unwrap_or(call).to_uppercase();
        let cq = self.conn.query_row(
            "SELECT cqframe_type_id, cqframe_time, frequency, snr, COALESCE(band, '') FROM cqframe
             WHERE upper(from_callsign) = ?1 AND snr IS NOT NULL ORDER BY cqframe_time DESC LIMIT 1",
            params![base],
            |r| Ok(crate::varac::messages::HeardEntry {
                callsign: base.clone(),
                source: if r.get::<_, i32>(0)? == 2 { "BEACON" } else { "CQ" }.into(),
                time: parse_dt(&r.get::<_, String>(1)?),
                frequency_hz: r.get(2)?,
                snr: r.get(3)?,
                band: r.get(4)?,
            }),
        );
        // (as in VarAC: CQ frames, broadcasts and QSOs, pings included)
        let bc = self.conn.query_row(
            "SELECT broadcast_time, frequency, snr, COALESCE(band, '') FROM broadcast
             WHERE upper(from_callsign) = ?1 AND snr IS NOT NULL ORDER BY broadcast_time DESC LIMIT 1",
            params![base],
            |r| Ok(crate::varac::messages::HeardEntry {
                callsign: base.clone(),
                source: "BROADCAST".into(),
                time: parse_dt(&r.get::<_, String>(0)?),
                frequency_hz: r.get(1)?,
                snr: r.get(2)?,
                band: r.get(3)?,
            }),
        );
        let qso = self.conn.query_row(
            "SELECT starttime, frequency, snr_received, COALESCE(band, '') FROM qso
             WHERE upper(callsign) = ?1 AND snr_received IS NOT NULL ORDER BY starttime DESC LIMIT 1",
            params![base],
            |r| Ok(crate::varac::messages::HeardEntry {
                callsign: base.clone(),
                source: "QSO".into(),
                time: parse_dt(&r.get::<_, String>(0)?),
                frequency_hz: r.get(1)?,
                snr: r.get(2)?,
                band: r.get(3)?,
            }),
        );
        let pick = |r: rusqlite::Result<crate::varac::messages::HeardEntry>| match r {
            Ok(e) => Ok(Some(e)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        };
        Ok([pick(cq)?, pick(bc)?, pick(qso)?].into_iter().flatten().max_by_key(|e| e.time))
    }

    pub fn recent_cqframes(&self, limit: usize) -> SqlResult<Vec<CqFrameRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT guid, cqframe_time, cqframe_type_id, frequency, bandwidth,
                    from_callsign, snr, slot, locator, band
             FROM cqframe ORDER BY cqframe_time DESC LIMIT ?1"
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(CqFrameRecord {
                guid: row.get(0)?,
                cqframe_time: parse_dt(&row.get::<_, String>(1)?),
                cqframe_type_id: row.get(2)?,
                frequency: row.get(3)?,
                bandwidth: row.get(4)?,
                from_callsign: row.get(5)?,
                snr: row.get(6)?,
                slot: row.get(7)?,
                locator: row.get(8)?,
                band: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    // === Contacts / Address Book ===

    pub fn upsert_contact(&self, contact: &ContactRecord) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO contacts (callsign, name, qth, locator, notes)
             VALUES (UPPER(?1), ?2, ?3, ?4, ?5)
             ON CONFLICT(callsign) DO UPDATE SET
               name = COALESCE(?2, name),
               qth = COALESCE(?3, qth),
               locator = COALESCE(?4, locator),
               notes = COALESCE(?5, notes)",
            params![
                contact.callsign,
                contact.name,
                contact.qth,
                contact.locator,
                contact.notes,
            ],
        )?;
        Ok(())
    }

    pub fn get_contact(&self, callsign: &str) -> SqlResult<Option<ContactRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.callsign, c.name, c.qth, c.locator, c.notes,
                    MAX(q.starttime), COUNT(q.guid)
             FROM contacts c
             LEFT JOIN qso q ON UPPER(q.callsign) = c.callsign
             WHERE c.callsign = UPPER(?1)
             GROUP BY c.callsign"
        )?;
        let mut rows = stmt.query_map(params![callsign], |row| {
            Ok(ContactRecord {
                callsign: row.get(0)?,
                name: row.get(1)?,
                qth: row.get(2)?,
                locator: row.get(3)?,
                notes: row.get(4)?,
                last_qso: row.get::<_, Option<String>>(5)?.map(|s| parse_dt(&s)),
                qso_count: row.get(6)?,
            })
        })?;
        rows.next().transpose()
    }

    pub fn all_contacts(&self) -> SqlResult<Vec<ContactRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.callsign, c.name, c.qth, c.locator, c.notes,
                    MAX(q.starttime), COUNT(q.guid)
             FROM contacts c
             LEFT JOIN qso q ON UPPER(q.callsign) = c.callsign
             GROUP BY c.callsign
             ORDER BY c.callsign ASC"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ContactRecord {
                callsign: row.get(0)?,
                name: row.get(1)?,
                qth: row.get(2)?,
                locator: row.get(3)?,
                notes: row.get(4)?,
                last_qso: row.get::<_, Option<String>>(5)?.map(|s| parse_dt(&s)),
                qso_count: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    pub fn delete_contact(&self, callsign: &str) -> SqlResult<()> {
        self.conn.execute(
            "DELETE FROM contacts WHERE callsign = UPPER(?1)",
            params![callsign],
        )?;
        Ok(())
    }

    pub fn auto_save_contact_from_qso(&self, callsign: &str, name: Option<&str>, qth: Option<&str>, locator: Option<&str>) -> SqlResult<()> {
        self.conn.execute(
            "INSERT INTO contacts (callsign, name, qth, locator)
             VALUES (UPPER(?1), ?2, ?3, ?4)
             ON CONFLICT(callsign) DO UPDATE SET
               name = COALESCE(?2, name),
               qth = COALESCE(?3, qth),
               locator = COALESCE(?4, locator)",
            params![callsign, name, qth, locator],
        )?;
        Ok(())
    }

    pub fn search_contacts(&self, query: &str) -> SqlResult<Vec<ContactRecord>> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT c.callsign, c.name, c.qth, c.locator, c.notes,
                    MAX(q.starttime), COUNT(q.guid)
             FROM contacts c
             LEFT JOIN qso q ON UPPER(q.callsign) = c.callsign
             WHERE c.callsign LIKE UPPER(?1)
                OR c.name LIKE ?1
                OR c.qth LIKE ?1
                OR c.notes LIKE ?1
             GROUP BY c.callsign
             ORDER BY c.callsign ASC"
        )?;
        let rows = stmt.query_map(params![pattern], |row| {
            Ok(ContactRecord {
                callsign: row.get(0)?,
                name: row.get(1)?,
                qth: row.get(2)?,
                locator: row.get(3)?,
                notes: row.get(4)?,
                last_qso: row.get::<_, Option<String>>(5)?.map(|s| parse_dt(&s)),
                qso_count: row.get(6)?,
            })
        })?;
        rows.collect()
    }

    // === Config key-value store ===

    pub fn get_config(&self, key: &str) -> SqlResult<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT value FROM config WHERE key = ?1"
        )?;
        let mut rows = stmt.query_map(params![key], |r| r.get(0))?;
        rows.next().transpose()
    }

    pub fn set_config(&self, key: &str, value: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO config (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn export_adif(&self) -> SqlResult<String> {
        let qsos = self.recent_qsos(10000)?;
        let mut out = String::new();
        out.push_str("ADIF export from Openslot\n");
        out.push_str(&format!("<ADIF_VER:5>3.1.4\n"));
        out.push_str(&format!("<PROGRAMID:8>Openslot\n"));
        out.push_str(&format!("<PROGRAMVERSION:{}>{}",
            env!("CARGO_PKG_VERSION").len(),
            env!("CARGO_PKG_VERSION")));
        out.push_str("\n<EOH>\n\n");

        for qso in &qsos {
            let date = qso.starttime.format("%Y%m%d").to_string();
            let time = qso.starttime.format("%H%M%S").to_string();
            out.push_str(&adif_field("QSO_DATE", &date));
            out.push_str(&adif_field("TIME_ON", &time));
            out.push_str(&adif_field("CALL", &qso.callsign));
            out.push_str(&adif_field("STATION_CALLSIGN", &qso.my_callsign));
            out.push_str(&adif_field("MODE", "VARA"));
            if !qso.band.is_empty() {
                out.push_str(&adif_field("BAND", &qso.band));
            }
            if qso.frequency > 0 {
                let freq_mhz = qso.frequency as f64 / 1_000_000.0;
                out.push_str(&adif_field("FREQ", &format!("{freq_mhz:.6}")));
            }
            if !qso.bandwidth.is_empty() {
                out.push_str(&adif_field("BANDWIDTH", &qso.bandwidth));
            }
            if let Some(end) = qso.endtime {
                let end_time = end.format("%H%M%S").to_string();
                out.push_str(&adif_field("TIME_OFF", &end_time));
            }
            if let Some(ref name) = qso.name {
                out.push_str(&adif_field("NAME", name));
            }
            if let Some(ref qth) = qso.qth {
                out.push_str(&adif_field("QTH", qth));
            }
            if let Some(snr) = qso.snr_received {
                out.push_str(&adif_field("SRX", &snr.to_string()));
            }
            if let Some(snr) = qso.snr_sent {
                out.push_str(&adif_field("STX", &snr.to_string()));
            }
            if let Some(ref comments) = qso.comments {
                out.push_str(&adif_field("COMMENT", comments));
            }
            if let Ok(Some(contact)) = self.get_contact(&qso.callsign) {
                if let Some(ref loc) = contact.locator {
                    if !loc.is_empty() {
                        out.push_str(&adif_field("GRIDSQUARE", loc));
                    }
                }
            }
            out.push_str("<EOR>\n\n");
        }

        Ok(out)
    }
}

pub fn parse_adif_contacts(content: &str) -> Vec<(String, Option<String>, Option<String>, Option<String>)> {
    let mut results = Vec::new();
    let upper = content.to_uppercase();
    let body_start = upper.find("<EOH>").map(|p| p + 5).unwrap_or(0);
    let body = &content[body_start..];

    let mut records = Vec::new();
    let mut remaining = body;
    loop {
        let lower = remaining.to_uppercase();
        if let Some(pos) = lower.find("<EOR>") {
            records.push(&remaining[..pos]);
            remaining = &remaining[pos + 5..];
        } else {
            break;
        }
    }

    for record in records {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        let call = extract_adif_field(record, "CALL");
        if let Some(call) = call {
            if !call.is_empty() {
                let name = extract_adif_field(record, "NAME");
                let qth = extract_adif_field(record, "QTH");
                let grid = extract_adif_field(record, "GRIDSQUARE");
                results.push((call.to_uppercase(), name, qth, grid));
            }
        }
    }
    results
}

pub fn parse_adif_qsos(content: &str, my_callsign: &str) -> Vec<QsoRecord> {
    let mut results = Vec::new();
    let upper = content.to_uppercase();
    let body_start = upper.find("<EOH>").map(|p| p + 5).unwrap_or(0);
    let body = &content[body_start..];

    let mut records = Vec::new();
    let mut remaining = body;
    loop {
        let lower = remaining.to_uppercase();
        if let Some(pos) = lower.find("<EOR>") {
            records.push(&remaining[..pos]);
            remaining = &remaining[pos + 5..];
        } else {
            break;
        }
    }

    for record in records {
        let record = record.trim();
        if record.is_empty() {
            continue;
        }
        let call = match extract_adif_field(record, "CALL") {
            Some(c) if !c.is_empty() => c.to_uppercase(),
            _ => continue,
        };

        let qso_date = extract_adif_field(record, "QSO_DATE").unwrap_or_default();
        let time_on = extract_adif_field(record, "TIME_ON").unwrap_or_default();
        let starttime = parse_adif_datetime(&qso_date, &time_on);

        let time_off = extract_adif_field(record, "TIME_OFF");
        let endtime = time_off.map(|t| parse_adif_datetime(&qso_date, &t));

        let freq_str = extract_adif_field(record, "FREQ").unwrap_or_default();
        let frequency = freq_str.parse::<f64>().map(|f| (f * 1_000_000.0) as i64).unwrap_or(0);

        let band = extract_adif_field(record, "BAND").unwrap_or_default();
        let bandwidth = extract_adif_field(record, "BANDWIDTH").unwrap_or_default();
        let name = extract_adif_field(record, "NAME");
        let qth = extract_adif_field(record, "QTH");
        let comments = extract_adif_field(record, "COMMENT")
            .or_else(|| extract_adif_field(record, "NOTES"));

        let snr_sent = extract_adif_field(record, "RST_SENT")
            .and_then(|s| s.parse::<i32>().ok());
        let snr_received = extract_adif_field(record, "RST_RCVD")
            .and_then(|s| s.parse::<i32>().ok());

        results.push(QsoRecord {
            guid: uuid::Uuid::new_v4().to_string(),
            starttime,
            endtime,
            frequency,
            band,
            bandwidth,
            callsign: call,
            my_callsign: my_callsign.to_string(),
            snr_received,
            snr_sent,
            name,
            qth,
            comments,
        });
    }
    results
}

fn parse_adif_datetime(date: &str, time: &str) -> DateTime<Utc> {
    let y: i32 = date.get(0..4).and_then(|s| s.parse().ok()).unwrap_or(2000);
    let m: u32 = date.get(4..6).and_then(|s| s.parse().ok()).unwrap_or(1);
    let d: u32 = date.get(6..8).and_then(|s| s.parse().ok()).unwrap_or(1);
    let h: u32 = time.get(0..2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let min: u32 = time.get(2..4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let s: u32 = time.get(4..6).and_then(|s| s.parse().ok()).unwrap_or(0);
    chrono::NaiveDate::from_ymd_opt(y, m, d)
        .and_then(|nd| nd.and_hms_opt(h, min, s))
        .map(|ndt| ndt.and_utc())
        .unwrap_or_else(Utc::now)
}

fn extract_adif_field(record: &str, field_name: &str) -> Option<String> {
    let upper = record.to_uppercase();
    let tag = format!("<{}:", field_name.to_uppercase());
    if let Some(pos) = upper.find(&tag) {
        let after_tag = &record[pos + tag.len()..];
        if let Some(gt) = after_tag.find('>') {
            let len_str = &after_tag[..gt];
            let field_len: usize = len_str.split(':').next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if field_len > 0 && after_tag.len() > gt + field_len {
                return Some(after_tag[gt + 1..gt + 1 + field_len].to_string());
            }
        }
    }
    None
}

fn row_to_vmail(row: &rusqlite::Row) -> rusqlite::Result<VMailRecord> {
    Ok(VMailRecord {
        guid: row.get(0)?,
        creation_time: parse_dt(&row.get::<_, String>(1)?),
        sent_time: row.get::<_, Option<String>>(2)?.map(|s| parse_dt(&s)),
        received_time: row.get::<_, Option<String>>(3)?.map(|s| parse_dt(&s)),
        folder_id: row.get(4)?,
        vmail_to: row.get(5)?,
        vmail_from: row.get(6)?,
        vmail_via: row.get(7)?,
        subject: row.get(8)?,
        msg: row.get(9)?,
        read_status: row.get(10)?,
        is_deleted: row.get(11)?,
        urgent: row.get(12)?,
        delivery_band: row.get(13)?,
        delivery_snr: row.get(14)?,
        frequency: row.get(15)?,
    })
}

fn fmt_dt(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| ndt.and_utc())
        .unwrap_or_default()
}

fn adif_field(name: &str, value: &str) -> String {
    // (VarAC counts UTF-16 units)
    format!("<{}:{}>{}  ", name, value.encode_utf16().count(), value)
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS cqframe_type (
    cqframe_type_id INTEGER PRIMARY KEY,
    cqframe_type TEXT NOT NULL
);
INSERT OR IGNORE INTO cqframe_type VALUES (1, 'CQ');
INSERT OR IGNORE INTO cqframe_type VALUES (2, 'Beacon');

CREATE TABLE IF NOT EXISTS vmail_folder (
    folder_id INTEGER PRIMARY KEY,
    folder TEXT NOT NULL
);
INSERT OR IGNORE INTO vmail_folder VALUES (1, 'Inbox');
INSERT OR IGNORE INTO vmail_folder VALUES (2, 'Sent');
INSERT OR IGNORE INTO vmail_folder VALUES (3, 'Outbox');
INSERT OR IGNORE INTO vmail_folder VALUES (4, 'Parking');

CREATE TABLE IF NOT EXISTS qso (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    starttime TEXT NOT NULL,
    endtime TEXT,
    frequency INTEGER NOT NULL,
    band TEXT NOT NULL,
    bandwidth TEXT NOT NULL,
    callsign TEXT NOT NULL,
    my_callsign TEXT NOT NULL,
    snr_received INTEGER,
    snr_sent INTEGER,
    name TEXT,
    qth TEXT,
    comments TEXT,
    varac_version TEXT,
    is_ping INTEGER DEFAULT 0,
    slot TEXT
);

CREATE TABLE IF NOT EXISTS datastream (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    datastream_entry_type_id INTEGER NOT NULL,
    qso_guid TEXT NOT NULL,
    callsign TEXT NOT NULL,
    entry TEXT NOT NULL,
    creation_time TEXT NOT NULL,
    file_path TEXT,
    chat_id INTEGER,
    reply_on_chat_id INTEGER,
    reaction_id INTEGER,
    FOREIGN KEY (qso_guid) REFERENCES qso(guid)
);

CREATE TABLE IF NOT EXISTS vmail (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    creation_time TEXT NOT NULL,
    sent_time TEXT,
    received_time TEXT,
    folder_id INTEGER NOT NULL DEFAULT 3,
    vmail_to TEXT NOT NULL,
    vmail_from TEXT NOT NULL,
    vmail_via TEXT,
    subject TEXT NOT NULL DEFAULT '',
    msg TEXT NOT NULL DEFAULT '',
    read_status INTEGER DEFAULT 0,
    is_deleted INTEGER DEFAULT 0,
    urgent INTEGER DEFAULT 0,
    delivery_band TEXT,
    delivery_snr INTEGER,
    frequency INTEGER,
    has_attachment INTEGER DEFAULT 0,
    FOREIGN KEY (folder_id) REFERENCES vmail_folder(folder_id)
);

-- Email VMail details (VarAC keeps these as vmail columns reply_to_email / raw_email)
CREATE TABLE IF NOT EXISTS vmail_email (
    vmail_guid TEXT PRIMARY KEY,
    reply_to TEXT,
    raw INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS vmail_attachment (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    vmail_guid TEXT NOT NULL,
    file_name TEXT NOT NULL,
    file_size_bytes INTEGER,
    creation_time TEXT NOT NULL,
    is_deleted INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS vmail_relay_notification (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    relay_notification_time TEXT NOT NULL,
    frequency INTEGER,
    from_callsign TEXT NOT NULL,
    is_deleted INTEGER DEFAULT 0
);

CREATE TABLE IF NOT EXISTS broadcast (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    broadcast_time TEXT NOT NULL,
    frequency INTEGER NOT NULL,
    from_callsign TEXT NOT NULL,
    to_callsign TEXT NOT NULL,
    via_callsign TEXT,
    broadcast_message TEXT NOT NULL,
    snr INTEGER,
    band TEXT
);

CREATE TABLE IF NOT EXISTS cqframe (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    cqframe_time TEXT NOT NULL,
    cqframe_type_id INTEGER NOT NULL,
    frequency INTEGER NOT NULL,
    bandwidth TEXT NOT NULL,
    from_callsign TEXT NOT NULL,
    snr INTEGER,
    slot INTEGER,
    locator TEXT,
    band TEXT,
    FOREIGN KEY (cqframe_type_id) REFERENCES cqframe_type(cqframe_type_id)
);

CREATE TABLE IF NOT EXISTS file_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_path TEXT NOT NULL,
    callsign TEXT,
    creation_time TEXT NOT NULL,
    outgoing INTEGER DEFAULT 0
);
CREATE TABLE IF NOT EXISTS qso_snr_report (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    qso_guid TEXT NOT NULL,
    snr_direction INTEGER NOT NULL,
    snr INTEGER NOT NULL,
    creation_time TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS alert (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guid TEXT UNIQUE NOT NULL,
    source_id INTEGER NOT NULL,
    from_callsign TEXT,
    to_callsign TEXT,
    alert_tag TEXT,
    alert_name TEXT,
    source_text TEXT,
    alert_time TEXT NOT NULL,
    read_status INTEGER DEFAULT 0,
    folder_id INTEGER DEFAULT 1,
    frequency INTEGER,
    is_deleted INTEGER DEFAULT 0
);
CREATE TABLE IF NOT EXISTS config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS contacts (
    callsign TEXT PRIMARY KEY,
    name TEXT,
    qth TEXT,
    locator TEXT,
    notes TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_qso_starttime ON qso(starttime);
CREATE INDEX IF NOT EXISTS idx_qso_callsign ON qso(callsign);
CREATE INDEX IF NOT EXISTS idx_datastream_qso ON datastream(qso_guid);
CREATE INDEX IF NOT EXISTS idx_datastream_time ON datastream(creation_time);
CREATE INDEX IF NOT EXISTS idx_vmail_folder ON vmail(folder_id, is_deleted);
CREATE INDEX IF NOT EXISTS idx_vmail_to ON vmail(vmail_to);
CREATE INDEX IF NOT EXISTS idx_broadcast_time ON broadcast(broadcast_time);
CREATE INDEX IF NOT EXISTS idx_cqframe_time ON cqframe(cqframe_time);
CREATE INDEX IF NOT EXISTS idx_cqframe_call ON cqframe(from_callsign);
";

#[cfg(test)]
mod tests {

    /// A database from before vmail_attachment existed (user_version 5, table missing)
    /// must gain the table on open.
    #[test]
    fn old_database_gets_new_tables() {
        let path = std::env::temp_dir().join(format!("old_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let db = Storage::open(&path).unwrap();
            db.conn.execute_batch("DROP TABLE vmail_attachment; PRAGMA user_version = 5;").unwrap();
        }
        let db = Storage::open(&path).unwrap();
        let f = std::env::temp_dir().join(format!("old_att_{}.txt", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        db.add_vmail_attachment("vm-1", &f).unwrap();
        assert_eq!(db.vmail_attachments("vm-1").unwrap(), vec![f.clone()]);
        let _ = std::fs::remove_file(f);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn relay_notifications_one_row_per_station() {
        let db = test_db();
        let t = Utc::now();
        db.record_relay_notification("NR5P-9", 14_105_000, false, &t).unwrap();
        db.record_relay_notification("NR5P", 7_105_000, true, &t).unwrap();
        db.record_relay_notification("K1ABC", 0, false, &t).unwrap();
        let rows = db.relay_notifications().unwrap();
        assert_eq!(rows.len(), 2);
        let nr5p = rows.iter().find(|r| r.2 == "NR5P").unwrap();
        assert_eq!((nr5p.1, nr5p.3), (7_105_000, true), "the latest one counts");
        db.clear_relay_notifications("nr5p").unwrap();
        assert_eq!(db.relay_notifications().unwrap().len(), 1);
    }

    #[test]
    fn vmail_attachments_are_stored_per_vmail() {
        let db = Storage::open_in_memory().unwrap();
        let f = std::env::temp_dir().join(format!("att_{}.txt", std::process::id()));
        std::fs::write(&f, b"hello").unwrap();
        db.add_vmail_attachment("vm-1", &f).unwrap();
        assert_eq!(db.vmail_attachments("vm-1").unwrap(), vec![f.clone()]);
        assert!(db.vmail_attachments("vm-2").unwrap().is_empty());
        let _ = std::fs::remove_file(f);
    }

    use super::*;

    fn test_db() -> Storage {
        Storage::open_in_memory().unwrap()
    }

    fn make_guid() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        format!("{:x}-{:x}", t.as_secs(), t.subsec_nanos())
    }

    #[test]
    fn schema_creates_cleanly() {
        let _db = test_db();
    }

    #[test]
    fn qso_roundtrip() {
        let db = test_db();
        let guid = make_guid();
        let now = Utc::now();
        let qso = QsoRecord {
            guid: guid.clone(),
            starttime: now,
            endtime: None,
            frequency: 14_105_000,
            band: "20m".into(),
            bandwidth: "2300".into(),
            callsign: "W1ABC".into(),
            my_callsign: "N0CALL".into(),
            snr_received: Some(-5),
            snr_sent: Some(3),
            name: Some("John".into()),
            qth: Some("Boston".into()),
            comments: None,
        };
        db.insert_qso(&qso).unwrap();

        let loaded = db.get_qso(&guid).unwrap().unwrap();
        assert_eq!(loaded.callsign, "W1ABC");
        assert_eq!(loaded.frequency, 14_105_000);
        assert_eq!(loaded.snr_received, Some(-5));

        db.update_qso_end(&guid, &now).unwrap();
        let loaded = db.get_qso(&guid).unwrap().unwrap();
        assert!(loaded.endtime.is_some());
    }

    #[test]
    fn chat_insert_and_history() {
        let db = test_db();
        let qso_guid = make_guid();
        let now = Utc::now();
        db.insert_qso(&QsoRecord {
            guid: qso_guid.clone(),
            starttime: now,
            endtime: None,
            frequency: 7_105_000,
            band: "40m".into(),
            bandwidth: "500".into(),
            callsign: "4Z1AC".into(),
            my_callsign: "N0CALL".into(),
            snr_received: None,
            snr_sent: None,
            name: None,
            qth: None,
            comments: None,
        }).unwrap();

        db.insert_chat(&ChatRecord {
            guid: make_guid(),
            entry_type: 1,
            qso_guid: qso_guid.clone(),
            callsign: "4Z1AC".into(),
            entry: "Hello!".into(),
            creation_time: now,
            chat_id: Some(1),
            reply_on_chat_id: None,
        }).unwrap();

        db.insert_chat(&ChatRecord {
            guid: make_guid(),
            entry_type: 2,
            qso_guid: qso_guid.clone(),
            callsign: "N0CALL".into(),
            entry: "Hi there!".into(),
            creation_time: now,
            chat_id: Some(2),
            reply_on_chat_id: Some(1),
        }).unwrap();

        let history = db.chat_history(&qso_guid, 10).unwrap();
        assert_eq!(history.len(), 2);
    }

    #[test]
    fn vmail_crud() {
        let db = test_db();
        let guid = make_guid();
        let now = Utc::now();

        db.insert_vmail(&VMailRecord {
            guid: guid.clone(),
            creation_time: now,
            sent_time: None,
            received_time: Some(now),
            folder_id: VMailFolder::Inbox as i32,
            vmail_to: "N0CALL".into(),
            vmail_from: "W1ABC".into(),
            vmail_via: None,
            subject: "Test".into(),
            msg: "Hello world".into(),
            read_status: false,
            is_deleted: false,
            urgent: false,
            delivery_band: Some("20m".into()),
            delivery_snr: Some(-8),
            frequency: Some(14_105_000),
        }).unwrap();

        assert_eq!(db.unread_vmail_count().unwrap(), 1);

        let inbox = db.vmails_in_folder(VMailFolder::Inbox, 10).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].subject, "Test");

        db.mark_vmail_read(&guid).unwrap();
        assert_eq!(db.unread_vmail_count().unwrap(), 0);

        db.mark_vmail_deleted(&guid).unwrap();
        let inbox = db.vmails_in_folder(VMailFolder::Inbox, 10).unwrap();
        assert_eq!(inbox.len(), 0);
    }

    #[test]
    fn vmail_outbox_workflow() {
        let db = test_db();
        let guid = make_guid();
        let now = Utc::now();

        db.insert_vmail(&VMailRecord {
            guid: guid.clone(),
            creation_time: now,
            sent_time: None,
            received_time: None,
            folder_id: VMailFolder::Outbox as i32,
            vmail_to: "4Z1AC".into(),
            vmail_from: "N0CALL".into(),
            vmail_via: None,
            subject: "Outgoing".into(),
            msg: "Send me".into(),
            read_status: false,
            is_deleted: false,
            urgent: true,
            delivery_band: None,
            delivery_snr: None,
            frequency: None,
        }).unwrap();

        let pending = db.pending_outbox_vmails().unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].urgent);

        db.mark_vmail_sent(&guid, &now).unwrap();
        let pending = db.pending_outbox_vmails().unwrap();
        assert_eq!(pending.len(), 0);

        let sent = db.vmails_in_folder(VMailFolder::Sent, 10).unwrap();
        assert_eq!(sent.len(), 1);
    }

    #[test]
    fn broadcast_dedup() {
        let db = test_db();
        let now = Utc::now();

        db.insert_broadcast(&BroadcastRecord {
            guid: make_guid(),
            broadcast_time: now,
            frequency: 14_105_000,
            from_callsign: "ON2AD".into(),
            to_callsign: "ALL".into(),
            via_callsign: None,
            broadcast_message: "any body QRV?".into(),
            snr: Some(-13),
            band: Some("20m".into()),
        }).unwrap();

        let is_dup = db.is_duplicate_broadcast(
            14_105_000, "ON2AD", "ALL", None, "any body QRV?", 300,
        ).unwrap();
        assert!(is_dup);

        let not_dup = db.is_duplicate_broadcast(
            14_105_000, "ON2AD", "ALL", None, "different msg", 300,
        ).unwrap();
        assert!(!not_dup);
    }

    #[test]
    fn cqframe_insert_and_query() {
        let db = test_db();
        let now = Utc::now();

        db.insert_cqframe(&CqFrameRecord {
            guid: make_guid(),
            cqframe_time: now,
            cqframe_type_id: 1,
            frequency: 14_105_000,
            bandwidth: "2300".into(),
            from_callsign: "W1ABC-3".into(),
            snr: Some(-18),
            slot: Some(3),
            locator: Some("FN42".into()),
            band: Some("20m".into()),
        }).unwrap();

        db.insert_cqframe(&CqFrameRecord {
            guid: make_guid(),
            cqframe_time: now,
            cqframe_type_id: 2,
            frequency: 14_105_000,
            bandwidth: "500".into(),
            from_callsign: "4Z1AC-9".into(),
            snr: Some(-12),
            slot: Some(9),
            locator: None,
            band: Some("20m".into()),
        }).unwrap();

        let frames = db.recent_cqframes(10).unwrap();
        assert_eq!(frames.len(), 2);
    }

    #[test]
    fn config_store() {
        let db = test_db();
        assert_eq!(db.get_config("my_call").unwrap(), None);

        db.set_config("my_call", "N0CALL").unwrap();
        assert_eq!(db.get_config("my_call").unwrap(), Some("N0CALL".into()));

        db.set_config("my_call", "W1ABC").unwrap();
        assert_eq!(db.get_config("my_call").unwrap(), Some("W1ABC".into()));
    }

    #[test]
    fn recent_qsos_ordered() {
        let db = test_db();
        for i in 0..5 {
            db.insert_qso(&QsoRecord {
                guid: format!("qso-{i}"),
                starttime: Utc::now(),
                endtime: None,
                frequency: 14_105_000,
                band: "20m".into(),
                bandwidth: "2300".into(),
                callsign: format!("CALL{i}"),
                my_callsign: "N0CALL".into(),
                snr_received: None,
                snr_sent: None,
                name: None,
                qth: None,
                comments: None,
            }).unwrap();
        }
        let recent = db.recent_qsos(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn contacts_crud() {
        let db = test_db();

        db.upsert_contact(&ContactRecord {
            callsign: "W1ABC".into(),
            name: Some("John".into()),
            qth: Some("Boston".into()),
            locator: Some("FN42".into()),
            notes: Some("Good operator".into()),
            last_qso: None,
            qso_count: 0,
        }).unwrap();

        let all = db.all_contacts().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].callsign, "W1ABC");
        assert_eq!(all[0].name.as_deref(), Some("John"));

        // Update existing contact
        db.auto_save_contact_from_qso("W1ABC", Some("John Smith"), None, Some("FN42")).unwrap();
        let contact = db.get_contact("W1ABC").unwrap().unwrap();
        assert_eq!(contact.name.as_deref(), Some("John Smith"));
        assert_eq!(contact.qth.as_deref(), Some("Boston")); // preserved
        assert_eq!(contact.locator.as_deref(), Some("FN42")); // added by auto_save

        // Search
        let found = db.search_contacts("john").unwrap();
        assert_eq!(found.len(), 1);
        let not_found = db.search_contacts("xyz").unwrap();
        assert_eq!(not_found.len(), 0);

        // Delete
        db.delete_contact("W1ABC").unwrap();
        assert!(db.get_contact("W1ABC").unwrap().is_none());
    }

    #[test]
    fn adif_import_parse() {
        let adif = r#"ADIF export
<ADIF_VER:5>3.1.4
<EOH>

<CALL:5>W1ABC  <NAME:4>John  <QTH:6>Boston  <GRIDSQUARE:4>FN42  <MODE:4>VARA  <EOR>

<CALL:4>NR5P  <NAME:3>Bob  <EOR>

<CALL:5>4Z1AC  <EOR>
"#;
        let contacts = parse_adif_contacts(adif);
        assert_eq!(contacts.len(), 3);
        assert_eq!(contacts[0].0, "W1ABC");
        assert_eq!(contacts[0].1.as_deref(), Some("John"));
        assert_eq!(contacts[0].2.as_deref(), Some("Boston"));
        assert_eq!(contacts[0].3.as_deref(), Some("FN42"));
        assert_eq!(contacts[1].0, "NR5P");
        assert_eq!(contacts[1].1.as_deref(), Some("Bob"));
        assert!(contacts[1].2.is_none());
        assert!(contacts[1].3.is_none());
        assert_eq!(contacts[2].0, "4Z1AC");
    }

    #[test]
    fn adif_import_no_header() {
        let adif = "<CALL:5>W1ABC  <NAME:4>John  <EOR>\n<CALL:4>NR5P  <EOR>\n";
        let contacts = parse_adif_contacts(adif);
        assert_eq!(contacts.len(), 2);
    }

    #[test]
    fn adif_qso_import() {
        let adif = r#"ADIF export
<ADIF_VER:5>3.1.4
<EOH>

<CALL:5>W1ABC <QSO_DATE:8>20260101 <TIME_ON:6>143000 <TIME_OFF:6>144500 <FREQ:8>14.10500 <BAND:3>20m <NAME:4>John <RST_SENT:3>+05 <RST_RCVD:3>-03 <EOR>

<CALL:4>NR5P <QSO_DATE:8>20260215 <TIME_ON:4>0900 <FREQ:8>7.10500 <BAND:3>40m <COMMENT:9>Good QSO! <EOR>
"#;
        let qsos = parse_adif_qsos(adif, "K1XYZ");
        assert_eq!(qsos.len(), 2);
        assert_eq!(qsos[0].callsign, "W1ABC");
        assert_eq!(qsos[0].band, "20m");
        assert_eq!(qsos[0].frequency, 14_105_000);
        assert_eq!(qsos[0].snr_sent, Some(5));
        assert_eq!(qsos[0].snr_received, Some(-3));
        assert_eq!(qsos[0].name.as_deref(), Some("John"));
        assert!(qsos[0].endtime.is_some());
        assert_eq!(qsos[0].my_callsign, "K1XYZ");
        assert_eq!(qsos[1].callsign, "NR5P");
        assert_eq!(qsos[1].band, "40m");
        assert_eq!(qsos[1].comments.as_deref(), Some("Good QSO!"));
        assert!(qsos[1].endtime.is_none());
    }
}

#[cfg(test)]
mod feature_tables_tests {
    use super::*;

    #[test]
    fn alerts_files_snr_map() {
        let db = Storage::open_in_memory().unwrap();
        let now = Utc::now();
        db.insert_alert(&AlertRecord {
            guid: "a1".into(), source_id: 3, from_callsign: "NR5P".into(), to_callsign: "ALL".into(),
            alert_tag: "POTA".into(), alert_name: "Parks".into(), source_text: "POTA K-1234".into(),
            alert_time: now, read: false, folder_id: 1, frequency: 14_105_000,
        }).unwrap();
        let a = db.alerts(1).unwrap();
        assert_eq!(a.len(), 1);
        assert!((a[0].alert_time - now).num_seconds().abs() <= 1, "time kept");
        assert_eq!(db.unread_alert_count().unwrap(), 1);
        db.move_alert(Some("a1"), 1, 2).unwrap();
        assert_eq!(db.unread_alert_count().unwrap(), 0);
        assert_eq!(db.alerts(2).unwrap().len(), 1);
        db.delete_alert(None, 2).unwrap();
        assert!(db.alerts(2).unwrap().is_empty());

        db.log_file(std::path::Path::new("/tmp/x.jpg"), "NR5P", false).unwrap();
        let f = db.logged_files().unwrap();
        assert_eq!(f.len(), 1);
        assert!((f[0].2 - now).num_seconds().abs() <= 1);

        db.save_snr_series("q1", &[(now, 5.0), (now, 7.0)], &[(now, -3.0)]).unwrap();
        let (his, mine) = db.snr_series("q1").unwrap();
        assert_eq!((his.len(), mine.len()), (2, 1));
        assert_eq!(mine[0].1, -3.0);

        db.insert_cqframe(&CqFrameRecord {
            guid: "c1".into(), cqframe_time: now, cqframe_type_id: 2, frequency: 14_105_000, bandwidth: "500".into(),
            from_callsign: "NR5P".into(), snr: Some(3), slot: None, locator: Some("EM10".into()), band: Some("20m".into()),
        }).unwrap();
        let m = db.map_stations(60).unwrap();
        assert_eq!(m, vec![("NR5P".to_string(), "EM10".to_string(), 1)]);
        assert!(db.cqframes_from("NR5P", now - chrono::Duration::hours(1)).unwrap().len() == 1);
        // ALE: NR5P heard on two frequencies, best average first
        for (i, (f, snr)) in [(7_105_000i64, -12), (7_105_000, -8), (14_105_000, 2)].iter().enumerate() {
            db.insert_cqframe(&CqFrameRecord {
                guid: format!("a{i}"), cqframe_time: now, cqframe_type_id: 2, frequency: *f, bandwidth: "500".into(),
                from_callsign: "NR5P-9".into(), snr: Some(*snr), slot: None, locator: None, band: None,
            }).unwrap();
        }
        let p = db.ale_pass("NR5P", now - chrono::Duration::minutes(60), None, 0).unwrap();
        assert_eq!(p.iter().map(|r| r.0).collect::<Vec<_>>(), vec![14_105_000, 7_105_000]);
        assert_eq!(p[1].1, -10.0);
        assert_eq!(p[1].2, 2);
        assert_eq!(db.ale_pass("NR5P", now - chrono::Duration::minutes(60), None, 14_105_000).unwrap().len(), 1, "current frequency left out");
        let h = chrono::Timelike::hour(&now);
        assert_eq!(db.ale_pass("NR5P", now - chrono::Duration::days(3), Some([(h + 23) % 24, h, (h + 1) % 24]), 0).unwrap().len(), 2);
        assert!(db.ale_pass("NR5P", now - chrono::Duration::days(3), Some([(h + 5) % 24, (h + 6) % 24, (h + 7) % 24]), 0).unwrap().is_empty());
    }
}

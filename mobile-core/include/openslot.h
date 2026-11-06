// C interface to the Openslot core, for the iOS app (Swift imports it as OpenslotCore).
//
// Strings in are UTF-8 JSON; strings returned are malloc'd by the library: free each with
// openslot_free. The engine runs on its own threads; poll openslot_next_event from the UI.
//
// Config (openslot_start):
//   {"data_dir": "...", "callsign": "N0CALL", "name": "", "qth": "", "locator": "",
//    "host": "127.0.0.1", "cmd_port": 8300, "bandwidth": "500", "frequency": 14105000,
//    "allow_info": true, "allow_pings": true, "verbose_snr_auto_accept": false,
//    "welcome_message": "", "welcome_always": false, "email": "", "full_callsign": "",
//    "call_id_minutes": 10, "idle_disconnect_minutes": 5, "allow_last_heard": true,
//    "multi_band_last_heard": false}
// Commands (openslot_command), {"cmd": ...}:
//   connect_modem, disconnect_modem, connect{call}, ping{call}, disconnect, abort,
//   chat{text}, typing, cq{slot?, wait_secs?}, slot_return, beacon,
//   listen{mode: on|cq|off}, bandwidth{bw}, broadcast{text}, vmail{to, subject, body,
//   urgent}, away{text|null}, file_accept,
//   file_decline, qsy_accept, qsy_reject, snr_request, info_request, tune, tune_stop,
//   clear_tx_buffer, set_frequency{hz}, policy{any config field from allow_info on},
//   verbose_snr{on}, verbose_snr_answer{accept}, qsy_request{offset_hz},
//   qsy_invite{slot}, inquire{what: last_heard|last_heard_call|parked|locator|gps|
//   version|schedule, call?}, return_cf{hz}, connect{call, slot_hz?},
//   send_file{path, pct?, quality?}, file_cancel
//   (send_file offers the file in a QSO off a calling frequency, VarAC's way; a picture
//   with pct goes through the image shrinker at pct % and quality 0-50, default 50;
//   file_cancel withdraws the offer, or once the data is going drops the link: VARA can't
//   take back data it has)
//   (connect, ping, cq and beacon are refused at 2300/2750 Hz on a calling frequency;
//   a cq naming a VarAC slot is followed by a wait there, see slot_wait)
// Events (openslot_next_event), {"type": ...}:
//   modem{connected, error?}, connected{call, bandwidth, ping, time}, disconnected,
//   chat{from, text, outgoing, time}, system{text, time}, typing, opening{call, snr, name,
//   qth, locator}, cq{call, source, bandwidth, beacon, slot, slot_label?, slot_hz?, time},
//   broadcast{from, to, message, time}, vmail{from, subject, parked}, vmail_changed,
//   file_offer{name, size}, file_incoming{name}, file_saved{path}, file_receive_failed,
//   file_send{state: offered|sending|done|refused|failed|cancelled, name?, why?, bytes?,
//   compressed?, total?}, file_progress{outgoing, done, total},
//   qsy_request{offset_hz | freq_hz},
//   qsy{offset_hz}, ptt{on}, busy{on}, buffer{bytes}, snr{db}, snr_report{db},
//   verbose_snr{on}, verbose_snr_invite{call},
//   slot_wait{slot, label, hz, cf_hz, seconds}, slot_wait_end{why}, cf_return{cf_hz, why},
//   qsy_invite_request{cf_hz, on_cf}, qsy_back{cf_hz}, vmail_relay_alert{call},
//   log{text}
// Queries (openslot_query), {"q": ...}:
//   vmails{folder: inbox|sent|outbox|parking}, vmail_read{guid}, vmail_delete{guid},
//   unread, qsos, heard, chat_history{call}, adif,
//   file_info{path, bandwidth, snr?, pct?, quality?, minutes?} (no engine needed; a
//   picture without pct starts at the size that sends in about `minutes`, default 5,
//   0 = the largest VARA allows): {name, bytes,
//   compressed, fits, estimate, limit, image, w?, h?, shrunk?{pct, quality, name, bytes,
//   compressed, fits, estimate, w, h}} or {error}

#ifndef OPENSLOT_H
#define OPENSLOT_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

const char *openslot_version(void);

// Why the last openslot_start / openslot_command / openslot_modem_start returned false.
char *openslot_last_error(void);

bool openslot_start(const char *config_json);
void openslot_stop(void);
bool openslot_command(const char *command_json);
char *openslot_query(const char *query_json);
// The next event, or NULL when none is waiting.
char *openslot_next_event(void);
void openslot_free(char *s);

// The built-in XARA modem. args_json: xara's command-line options as a JSON array of strings.
bool openslot_modem_start(const char *args_json);
// Stops it (PTT is released first) and returns its exit status: 0 clean, 2 bad options, else 1.
uint8_t openslot_modem_stop(void);
bool openslot_modem_running(void);
// The next line the modem logged, or NULL.
char *openslot_modem_next_log(void);
// What the modem decoded since the last call (the links' text, and other stations' with the
// monitor on), or NULL.
char *openslot_modem_next_received(void);

#ifdef __cplusplus
}
#endif

#endif

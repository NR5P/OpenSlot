import AVFoundation
import Foundation
import OpenslotCore

/// The station settings, saved with @AppStorage under these keys.
enum Keys {
    static let call = "call", name = "name", qth = "qth", locator = "locator"
    static let modemSource = "modemSource"  // "builtin" (XARA on this phone) or "network"
    static let host = "host", port = "port", bandwidth = "bandwidth", frequency = "frequency"
    static let ptt = "ptt", rigctld = "rigctld", driveDb = "driveDb"
    static let allowBuiltIn = "allowBuiltIn", registered = "registered"
    static let allowInfo = "allowInfo", allowPings = "allowPings", verboseSnrAuto = "verboseSnrAuto"
    static let cqSlot = "cqSlot", cqSlotWait = "cqSlotWait", skipSlotSelector = "skipSlotSelector"
    static let welcome = "welcome", welcomeAlways = "welcomeAlways", email = "email", fullCall = "fullCall"
    static let callIdMinutes = "callIdMinutes", idleMinutes = "idleMinutes"
    static let allowLastHeard = "allowLastHeard", multiBandLastHeard = "multiBandLastHeard"
    static let myFrequencies = "myFrequencies"  // the picker's "My frequencies" (JSON)
    static let photoMinutes = "photoMinutes"  // a photo's starting size: about this many minutes to send (0: largest)
    // The Modem tab's other XARA settings, as the NR5P XARA app
    static let monitor = "monitor", monitorFrames = "monitorFrames", monitorCalls = "monitorCalls"
    static let debugLog = "debugLog", recordAudio = "recordAudio", lan = "lan", uiControl = "uiControl"
    static let uiToken = "uiToken"  // kept, so a bookmarked controlling web UI keeps working
}

/// VarAC's slot system: QSOs move to slots around the calling frequency, 750 Hz apart for
/// VARA HF; 1-5 are below it, 11-15 above, 0 is the calling frequency itself.
enum Slots {
    static let byFrequency = [5, 4, 3, 2, 1, 0, 11, 12, 13, 14, 15]
    static let step = 750

    static func offset(_ slot: Int) -> Int {
        switch slot {
        case 1...5: return -slot * step
        case 11...15: return (slot - 10) * step
        default: return 0
        }
    }

    static func label(_ slot: Int) -> String {
        slot == 0 ? "CF" : String(format: "slot %d (%+d Hz)", slot, offset(slot))
    }
}

/// MHz with the digits a frequency needs: 14.105, 14.1065, 14.10275.
func mhz(_ hz: Int) -> String {
    var s = String(format: "%.5f", Double(hz) / 1e6)
    while s.hasSuffix("0"), let dot = s.firstIndex(of: "."), s.distance(from: dot, to: s.endIndex) > 4 { s.removeLast() }
    return s
}

/// The dial readout as the desktop shows it: 14.105.000.
func dial(_ hz: Int) -> String {
    String(format: "%d.%03d.%03d", hz / 1_000_000, (hz / 1000) % 1000, hz % 1000)
}

/// A frequency the operator keeps in the picker, with an optional name.
struct SavedFrequency: Codable, Hashable {
    let hz: Int
    let name: String
}

/// One of VarAC's calling frequencies.
struct CallingFrequency: Hashable {
    let hz: Int
    let band: String
}

struct Settings {
    private static let d = UserDefaults.standard
    var call: String { (Self.d.string(forKey: Keys.call) ?? "").trimmingCharacters(in: .whitespaces).uppercased() }
    var name: String { Self.d.string(forKey: Keys.name) ?? "" }
    var qth: String { Self.d.string(forKey: Keys.qth) ?? "" }
    var locator: String { Self.d.string(forKey: Keys.locator) ?? "" }
    var builtIn: Bool { (Self.d.string(forKey: Keys.modemSource) ?? "builtin") == "builtin" }
    var host: String { builtIn ? "127.0.0.1" : (Self.d.string(forKey: Keys.host) ?? "192.168.1.10") }
    var port: Int { builtIn ? 8300 : (Self.d.object(forKey: Keys.port) as? Int ?? 8300) }
    var bandwidth: String { Self.d.string(forKey: Keys.bandwidth) ?? "500" }
    var frequency: Int { Self.d.object(forKey: Keys.frequency) as? Int ?? 14_105_000 }
    var ptt: String { Self.d.string(forKey: Keys.ptt) ?? "vox" }
    var rigctld: String { Self.d.string(forKey: Keys.rigctld) ?? "192.168.1.10:4532" }
    var driveDb: Double { Self.d.object(forKey: Keys.driveDb) as? Double ?? -5 }
    var allowBuiltIn: Bool { Self.d.bool(forKey: Keys.allowBuiltIn) }
    var photoMinutes: Int { Self.d.object(forKey: Keys.photoMinutes) as? Int ?? 5 }
    var registered: Bool { Self.d.bool(forKey: Keys.registered) }
    var monitor: Bool { Self.d.bool(forKey: Keys.monitor) }
    var monitorFrames: Bool { Self.d.bool(forKey: Keys.monitorFrames) }
    var monitorCalls: [String] {
        (Self.d.string(forKey: Keys.monitorCalls) ?? "").uppercased().split(whereSeparator: { $0 == " " || $0 == "," }).map(String.init)
    }
    var debugLog: Bool { Self.d.bool(forKey: Keys.debugLog) }
    var recordAudio: Bool { Self.d.bool(forKey: Keys.recordAudio) }
    var lan: Bool { Self.d.bool(forKey: Keys.lan) }
    var uiControl: Bool { Self.d.bool(forKey: Keys.uiControl) }
    /// The web UI's token (made the first time it's needed).
    var uiToken: String {
        if let t = Self.d.string(forKey: Keys.uiToken), t.range(of: #"^[A-Za-z0-9_-]{8,128}$"#, options: .regularExpression) != nil {
            return t
        }
        let t = UUID().uuidString.replacingOccurrences(of: "-", with: "")
        Self.d.set(t, forKey: Keys.uiToken)
        return t
    }
    /// The modem's web UI, with the token that lets it control the radio when that's on.
    var webURL: URL { URL(string: "http://127.0.0.1:8080/" + (uiControl ? "?token=\(uiToken)" : ""))! }
    var allowInfo: Bool { Self.d.object(forKey: Keys.allowInfo) as? Bool ?? true }
    var allowPings: Bool { Self.d.object(forKey: Keys.allowPings) as? Bool ?? true }
    var verboseSnrAuto: Bool { Self.d.bool(forKey: Keys.verboseSnrAuto) }
    /// The slot last chosen in the CQ slot selector (VarAC remembers it)
    var cqSlot: Int { Self.d.object(forKey: Keys.cqSlot) as? Int ?? 11 }
    var cqSlotWait: Int { Self.d.object(forKey: Keys.cqSlotWait) as? Int ?? 300 }
    var skipSlotSelector: Bool { Self.d.bool(forKey: Keys.skipSlotSelector) }

    var welcome: String { Self.d.string(forKey: Keys.welcome) ?? "" }
    var welcomeAlways: Bool { Self.d.bool(forKey: Keys.welcomeAlways) }
    var email: String { Self.d.string(forKey: Keys.email) ?? "" }
    var fullCall: String { (Self.d.string(forKey: Keys.fullCall) ?? "").trimmingCharacters(in: .whitespaces).uppercased() }
    var callIdMinutes: Int { Self.d.object(forKey: Keys.callIdMinutes) as? Int ?? 10 }
    var idleMinutes: Int { Self.d.object(forKey: Keys.idleMinutes) as? Int ?? 5 }
    var allowLastHeard: Bool { Self.d.object(forKey: Keys.allowLastHeard) as? Bool ?? true }
    var multiBandLastHeard: Bool { Self.d.bool(forKey: Keys.multiBandLastHeard) }

    /// Settings → QSO and profile, as the engine's `policy` command and config take them.
    var policy: [String: Any] {
        ["allow_info": allowInfo, "allow_pings": allowPings, "verbose_snr_auto_accept": verboseSnrAuto,
         "welcome_message": welcome, "welcome_always": welcomeAlways, "email": email, "full_callsign": fullCall,
         "call_id_minutes": callIdMinutes, "idle_disconnect_minutes": idleMinutes,
         "allow_last_heard": allowLastHeard, "multi_band_last_heard": multiBandLastHeard]
    }

    var callValid: Bool {
        call.range(of: #"^[A-Z0-9]{3,7}(-[0-9]{1,2})?$"#, options: .regularExpression) != nil
            && call.contains(where: \.isNumber) && !call.hasPrefix("N0CALL")
    }

    static var documents: URL { FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0] }
    static var modemState: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("xara")
    }
    /// The modem's debug log and recordings (the Files app shows them under Openslot › xara-logs).
    static var modemLogs: URL { documents.appendingPathComponent("xara-logs") }

    var engineConfig: [String: Any] {
        ["data_dir": Self.documents.path, "callsign": call, "name": name, "qth": qth, "locator": locator,
         "host": host, "cmd_port": port, "bandwidth": bandwidth, "frequency": frequency]
            .merging(policy) { a, _ in a }
    }

    /// XARA's options: audio through the current iOS route (the radio's USB interface).
    var modemArguments: [String] {
        var a = ["--call", call, "--hf", bandwidth]
        a += ["--capture", "route", "--playback", "route", "--radio"]
        a += ["--ptt", ptt == "rigctld" ? "rigctld:\(rigctld)" : "vox"]
        a += ["--drive-db", String(format: "%.1f", driveDb)]
        // On the local network too when chosen (the engine still connects on 127.0.0.1)
        let bind = lan ? "0.0.0.0" : "127.0.0.1"
        a += ["--addr", bind, "--port", "8300", "--kiss-port", "8100"]
        a += ["--ui-port", "8080", "--ui-addr", bind]
        if registered { a += ["--registered"] }
        if monitor {
            a += ["--monitor"]
            if monitorFrames { a += ["--monitor-frames"] }
            for c in monitorCalls { a += ["--monitor-call", c] }
        }
        if uiControl { a += ["--ui-control", "--ui-token", uiToken] }
        if debugLog || recordAudio {
            try? FileManager.default.createDirectory(at: Self.modemLogs, withIntermediateDirectories: true)
        }
        if debugLog { a += ["--debug-log", Self.modemLogs.appendingPathComponent("debug.log").path] }
        if recordAudio {
            let stamp = Int(Date().timeIntervalSince1970)
            a += ["--record", Self.modemLogs.appendingPathComponent("xara-\(stamp).flac").path]
        }
        a += ["--state-dir", Self.modemState.path]
        return a
    }
}

struct ChatLine: Identifiable {
    let id = UUID()
    let from: String
    let text: String
    let time: String
    let outgoing: Bool
    let system: Bool
}

struct Heard: Identifiable {
    var id: String { call }
    let call: String
    var time: String
    var bandwidth: String
    var beacon: Bool
    var count: Int
    /// The slot a CQ's caller waits on (VarAC: 1-5 below the calling frequency, 11-15
    /// above; 0 = none), with its label and frequency.
    var slot = 0
    var slotLabel = ""
    var slotHz: Int?

    mutating func update(slotFrom e: [String: Any]) {
        slot = e["slot"] as? Int ?? 0
        slotLabel = e["slot_label"] as? String ?? ""
        slotHz = e["slot_hz"] as? Int
    }
}

/// Standing by on our slot after a CQ, until a reply comes or `until`.
struct SlotWait {
    let slot: Int
    let label: String
    let hz: Int
    let cfHz: Int
    let until: Date
}

/// A QSY invitation waiting for the operator (ours or the partner's): move to `targetHz`.
struct QsyInvitation {
    let targetHz: Int
    let inviter: Bool
    let seconds: Int
}

struct BroadcastLine: Identifiable {
    let id = UUID()
    let from: String
    let to: String
    let message: String
    let time: String
}

struct VMailItem: Identifiable, Hashable {
    let id: String
    let from: String
    let to: String
    let subject: String
    let body: String
    let time: String
    var read: Bool
    let urgent: Bool
}

/// A file going over the link: ours (`outgoing`) or the partner's. `total` is 0 while our
/// offer waits for the partner's answer.
struct FileTransfer: Equatable {
    var name: String
    var outgoing: Bool
    var done: Int
    var total: Int
    /// When the data started moving and how much was there then, for the time left
    var started: Date?
    var startDone = 0
    var fraction: Double { total > 0 ? min(1, Double(done) / Double(total)) : 0 }
    /// Seconds to go at the speed so far (nil until there's enough to tell)
    var timeLeft: TimeInterval? {
        guard let started, done > startDone, done < total else { return nil }
        let secs = Date().timeIntervalSince(started)
        guard secs >= 5 else { return nil }
        return Double(total - done) / (Double(done - startDone) / secs)
    }
}

/// The Rust core (protocol, session, storage) and the built-in XARA modem, as the UI sees them.
@MainActor
final class Core: ObservableObject {
    @Published private(set) var engineRunning = false
    @Published private(set) var modemRunning = false
    @Published private(set) var modemConnected = false
    @Published private(set) var status = "Stopped"
    @Published private(set) var partner: String?
    @Published private(set) var linkBandwidth = ""
    @Published private(set) var partnerInfo = ""
    @Published private(set) var ptt = false
    @Published private(set) var busy = false
    @Published private(set) var partnerTyping = false
    @Published private(set) var chat: [ChatLine] = []
    @Published private(set) var heard: [Heard] = []
    @Published private(set) var broadcasts: [BroadcastLine] = []
    @Published private(set) var log: [String] = []
    /// What the modem decoded (the Modem tab's Received page).
    @Published private(set) var received = ""
    @Published private(set) var unread = 0
    @Published private(set) var vmailRevision = 0
    @Published var fileOffer: String?
    /// A file going out or coming in on this link (the Chat screen's transfer bar)
    @Published private(set) var transfer: FileTransfer?
    /// VARA's last SNR report, for the send sheet's time estimate
    private(set) var lastSnr = 0
    private var incomingName: String?
    @Published var qsyRequest: String?
    /// Verbose SNR is running on this link (an SNR report each minute)
    @Published private(set) var verboseSnr = false
    /// The partner invited us to verbose SNR: answer with answerVerboseSnr
    @Published var verboseSnrInvite: String?
    /// The CQ slot selector is open
    @Published var showCqSelector = false
    @Published private(set) var slotWait: SlotWait?
    /// We called and the partner allows auto-QSY: pick the slot (the calling frequency in Hz)
    @Published var qsyInviteFrom: Int?
    private(set) var qsyInviteOnCf = true
    /// When the picker stays by itself, as the desktop's does after 80 s
    private(set) var qsyInviteUntil: Date?
    /// "Your QSO is over. Back to the calling frequency?" (its frequency in Hz)
    @Published var qsyBack: Int?
    /// VarAC's "QSY NOW" countdown: answer with answerQsy (rejected by itself when it runs out)
    @Published var qsyInvitation: QsyInvitation?
    /// We hold VMail for the partner but the link is on the calling frequency
    @Published var vmailRelayAlert: String?
    /// VarAC's "Bandwidth not allowed!": a wide link was refused on a calling frequency
    @Published var bwRefused = false
    @Published private(set) var route = AudioRoute.describe()

    private var timer: Timer?
    private var typingUntil = Date.distantPast
    private var connectDeadline: Date?
    private var resumeAfterInterruption = false

    static var version: String { String(cString: openslot_version()) }

    init() {
        timer = Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.tick() }
        }
        let nc = NotificationCenter.default
        nc.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.routeChanged() }
        }
        nc.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] n in
            let info = n.userInfo ?? [:]
            let type = (info[AVAudioSessionInterruptionTypeKey] as? UInt).flatMap(AVAudioSession.InterruptionType.init)
            let opts = AVAudioSession.InterruptionOptions(rawValue: info[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0)
            MainActor.assumeIsolated { self?.interrupted(type, resume: opts.contains(.shouldResume)) }
        }
        startEngine()
    }

    // MARK: engine

    /// (Re)starts the engine with the saved station settings.
    func startEngine() {
        let s = Settings()
        guard s.callValid else {
            status = "Set your call sign in Settings."
            return
        }
        guard let data = try? JSONSerialization.data(withJSONObject: s.engineConfig),
              let json = String(data: data, encoding: .utf8) else { return }
        if openslot_start(json) {
            engineRunning = true
            unread = (query(["q": "unread"]) as? Int) ?? 0
            loadHeardHistory()
        } else {
            status = "Couldn't start: \(lastError())"
        }
    }

    private func lastError() -> String {
        let p = openslot_last_error()
        defer { openslot_free(p) }
        return p.map { String(cString: $0) } ?? ""
    }

    func send(_ cmd: [String: Any]) {
        guard let data = try? JSONSerialization.data(withJSONObject: cmd),
              let json = String(data: data, encoding: .utf8) else { return }
        if !openslot_command(json) { addSystem(lastError()) }
    }

    func query(_ q: [String: Any]) -> Any? {
        guard let data = try? JSONSerialization.data(withJSONObject: q),
              let json = String(data: data, encoding: .utf8),
              let out = openslot_query(json) else { return nil }
        defer { openslot_free(out) }
        return try? JSONSerialization.jsonObject(with: Data(String(cString: out).utf8), options: [.fragmentsAllowed])
    }

    // MARK: going on the air

    /// Starts the modem (XARA here, or connects to the network one) and the link to it.
    func goOnline() async {
        let s = Settings()
        guard s.callValid else {
            status = "Set your call sign in Settings."
            return
        }
        if !engineRunning { startEngine() }
        if s.builtIn && !modemRunning {
            guard await startModem(s) else { return }
        }
        status = "Connecting to the modem…"
        connectDeadline = Date().addingTimeInterval(15)
        send(["cmd": "connect_modem"])
    }

    /// The Modem tab's settings changed: restart the engine and modem with them, and go back on
    /// the air if it was.
    func restartWithSettings() async {
        let wasOnline = modemConnected || modemRunning
        await goOffline()
        startEngine()
        if wasOnline { await goOnline() }
    }

    func clearReceived() { received = "" }
    func clearLog() { log = [] }

    func goOffline() async {
        connectDeadline = nil
        send(["cmd": "disconnect_modem"])
        resumeAfterInterruption = false
        await stopModem()
        status = "Offline"
    }

    private func startModem(_ s: Settings) async -> Bool {
        guard await AudioRoute.requestMicrophone() else {
            status = "Microphone access is off. Allow it in Settings › Openslot."
            return false
        }
        do {
            try AudioRoute.activate()
        } catch {
            status = "Audio session: \(error.localizedDescription)"
            return false
        }
        route = AudioRoute.describe()
        if !s.allowBuiltIn, let why = AudioRoute.builtInReason() {
            status = "Not transmitting into \(why). Connect your radio's USB audio interface."
            return false
        }
        try? FileManager.default.createDirectory(at: Settings.modemState, withIntermediateDirectories: true)
        guard let data = try? JSONSerialization.data(withJSONObject: s.modemArguments),
              let json = String(data: data, encoding: .utf8) else { return false }
        guard openslot_modem_start(json) else {
            status = "Modem: \(lastError())"
            return false
        }
        modemRunning = true
        status = "XARA modem starting…"
        // Give it a moment to open its ports before the engine connects.
        try? await Task.sleep(for: .milliseconds(800))
        return true
    }

    private func stopModem() async {
        guard modemRunning else { return }
        let code = await Task.detached { openslot_modem_stop() }.value
        modemRunning = false
        if code != 0 { addLog("XARA stopped with exit \(code)") }
    }

    // MARK: actions

    func connect(_ call: String) { send(["cmd": "connect", "call": call]) }
    func ping(_ call: String) { send(["cmd": "ping", "call": call]) }
    func disconnect() { send(["cmd": "disconnect"]) }
    func abort() { send(["cmd": "abort"]) }
    /// Call CQ: VarAC first asks which slot to wait on afterwards, unless that's switched off.
    func cq() {
        let s = Settings()
        if s.skipSlotSelector { callCq(slot: 0) } else { showCqSelector = true }
    }

    func callCq(slot: Int) {
        UserDefaults.standard.set(slot, forKey: Keys.cqSlot)
        send(["cmd": "cq", "slot": slot, "wait_secs": Settings().cqSlotWait])
    }

    /// Stop waiting on the slot: back to the calling frequency.
    func returnToCf() { send(["cmd": "slot_return"]) }

    /// Our QSY invitation in the opening handshake (0 = stay on the calling frequency).
    func qsyInvite(slot: Int) {
        qsyInviteFrom = nil
        qsyInviteUntil = nil
        send(["cmd": "qsy_invite", "slot": slot])
    }

    /// Ask the partner to QSY by `offsetHz` (e.g. to relay VMail off the calling frequency).
    func qsyRequest(_ offsetHz: Int) { send(["cmd": "qsy_request", "offset_hz": offsetHz]) }

    /// The operator's answer in the "QSY NOW" countdown.
    func answerQsy(_ accept: Bool) {
        qsyInvitation = nil
        send(["cmd": "qsy_answer", "accept": accept])
    }

    /// After the QSO: the operator retuned to the calling frequency.
    func backToCf(_ hz: Int) {
        qsyBack = nil
        send(["cmd": "return_cf", "hz": hz])
    }

    /// VarAC's INQUIRE: last_heard, last_heard_call (with `call`), parked, locator, gps,
    /// version, schedule; "info" asks for the partner's station info.
    func inquire(_ what: String, call: String? = nil) {
        if what == "info" { return send(["cmd": "info_request"]) }
        var c: [String: Any] = ["cmd": "inquire", "what": what]
        if let call { c["call"] = call }
        send(c)
    }
    func beacon() { send(["cmd": "beacon"]) }
    func sendChat(_ text: String) { send(["cmd": "chat", "text": text]) }
    func setVerboseSnr(_ on: Bool) { send(["cmd": "verbose_snr", "on": on]) }

    func answerVerboseSnr(_ accept: Bool) {
        verboseSnrInvite = nil
        send(["cmd": "verbose_snr_answer", "accept": accept])
    }

    /// Settings → QSO changed: applies at once (MYCALL is re-registered for pings).
    func applyPolicy() {
        guard engineRunning else { return }
        send(["cmd": "policy"].merging(Settings().policy) { a, _ in a })
    }

    /// Answer a CQ that names a slot (VarAC): the operator has tuned the radio there; the
    /// engine logs that frequency and goes back to the calling one after the QSO.
    func connectOnSlot(_ h: Heard) {
        guard let hz = h.slotHz else { return connect(h.call) }
        send(["cmd": "connect", "call": h.call, "slot_hz": hz])
    }

    // MARK: files

    /// The send sheet's preview of a file (the engine's file_info): sizes, whether it fits
    /// the link, the time it takes, and for a picture the shrinker's result at `pct` (nil:
    /// the largest size that fits). Shrinking a photo takes a moment: run off the main thread.
    nonisolated static func fileInfo(path: String, bandwidth: String, snr: Int, pct: Int?, quality: Int, minutes: Int) async -> [String: Any] {
        var q: [String: Any] = ["q": "file_info", "path": path, "bandwidth": bandwidth, "snr": snr, "quality": quality, "minutes": minutes]
        if let pct { q["pct"] = pct }
        guard let data = try? JSONSerialization.data(withJSONObject: q),
              let json = String(data: data, encoding: .utf8) else { return ["error": "The file couldn't be read."] }
        let answer: String = await Task.detached(priority: .userInitiated) {
            guard let out = openslot_query(json) else { return "" }
            defer { openslot_free(out) }
            return String(cString: out)
        }.value
        return (try? JSONSerialization.jsonObject(with: Data(answer.utf8))) as? [String: Any]
            ?? ["error": "The file couldn't be read."]
    }

    /// The bandwidth the link runs at (else the one set), for the file limits.
    var fileBandwidth: String { linkBandwidth.isEmpty ? Settings().bandwidth : linkBandwidth }

    /// Offer the partner a file; a picture with `pct` goes through the image shrinker.
    func sendFile(path: String, pct: Int?, quality: Int) {
        var c: [String: Any] = ["cmd": "send_file", "path": path]
        if let pct { c["pct"] = pct; c["quality"] = quality }
        send(c)
    }

    func cancelFile() { send(["cmd": "file_cancel"]) }

    func sendVMail(to: String, subject: String, body: String, urgent: Bool) {
        send(["cmd": "vmail", "to": to, "subject": subject, "body": body, "urgent": urgent])
    }

    // MARK: frequency and bandwidth (the main screen's picker and buttons)

    /// The operator tuned to `hz` (the phone doesn't tune the radio): log it and apply the
    /// calling-frequency rules there.
    func setFrequency(_ hz: Int) {
        UserDefaults.standard.set(hz, forKey: Keys.frequency)
        if engineRunning { send(["cmd": "set_frequency", "hz": hz]) }
        addSystem("Frequency \(mhz(hz)) MHz\(bandOf(hz).map { " (\($0))" } ?? ""): tune your radio there (USB)")
    }

    /// VARA HF bandwidth: switches the modem now when it's connected, else applies when it does.
    func setBandwidth(_ bw: String) {
        UserDefaults.standard.set(bw, forKey: Keys.bandwidth)
        if engineRunning { send(["cmd": "bandwidth", "bw": bw]) }
    }

    /// A frequency typed by the operator (MHz, kHz or Hz, as the desktop reads it), in Hz.
    func parseFrequency(_ text: String) -> Int? {
        query(["q": "parse_frequency", "text": text]) as? Int
    }

    private(set) lazy var callingFrequencies: [CallingFrequency] =
        (query(["q": "calling_frequencies"]) as? [[String: Any]] ?? []).map {
            CallingFrequency(hz: $0["hz"] as? Int ?? 0, band: $0["band"] as? String ?? "")
        }

    /// The ham band of `hz` ("20m"), or nil outside the bands.
    func bandOf(_ hz: Int) -> String? {
        (query(["q": "band", "hz": hz]) as? [String: Any])?["band"] as? String
    }

    func isCallingFrequency(_ hz: Int) -> Bool {
        (query(["q": "band", "hz": hz]) as? [String: Any])?["calling"] as? Bool ?? false
    }

    var myFrequencies: [SavedFrequency] {
        get {
            guard let data = UserDefaults.standard.data(forKey: Keys.myFrequencies) else { return [] }
            return (try? JSONDecoder().decode([SavedFrequency].self, from: data)) ?? []
        }
        set {
            UserDefaults.standard.set(try? JSONEncoder().encode(newValue.sorted { $0.hz < $1.hz }), forKey: Keys.myFrequencies)
        }
    }

    func vmails(_ folder: String) -> [VMailItem] {
        let list = query(["q": "vmails", "folder": folder]) as? [[String: Any]] ?? []
        return list.map {
            VMailItem(id: $0["guid"] as? String ?? UUID().uuidString, from: $0["from"] as? String ?? "",
                      to: $0["to"] as? String ?? "", subject: $0["subject"] as? String ?? "",
                      body: $0["body"] as? String ?? "", time: $0["time"] as? String ?? "",
                      read: $0["read"] as? Bool ?? true, urgent: $0["urgent"] as? Bool ?? false)
        }
    }

    func markRead(_ guid: String) {
        _ = query(["q": "vmail_read", "guid": guid])
        unread = (query(["q": "unread"]) as? Int) ?? 0
    }

    func delete(_ guid: String) {
        _ = query(["q": "vmail_delete", "guid": guid])
        unread = (query(["q": "unread"]) as? Int) ?? 0
        vmailRevision += 1
    }

    func clearChat() { chat.removeAll() }

    private func loadHeardHistory() {
        let list = query(["q": "heard", "limit": 300]) as? [[String: Any]] ?? []
        var seen: [String: Heard] = [:]
        for c in list {
            guard let call = c["call"] as? String, seen[call] == nil else { continue }
            var h = Heard(call: call, time: c["time"] as? String ?? "", bandwidth: c["bandwidth"] as? String ?? "",
                          beacon: c["beacon"] as? Bool ?? false, count: 1)
            h.update(slotFrom: c)
            seen[call] = h
        }
        heard = list.compactMap { $0["call"] as? String }.reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }
            .compactMap { seen[$0] }
    }

    // MARK: events

    private func tick() {
        while let p = openslot_next_event() {
            let s = String(cString: p)
            openslot_free(p)
            if let obj = try? JSONSerialization.jsonObject(with: Data(s.utf8)) as? [String: Any] {
                handle(obj)
            }
        }
        while let p = openslot_modem_next_log() {
            addLog(String(cString: p))
            openslot_free(p)
        }
        var heard = ""
        while let p = openslot_modem_next_received() {
            heard += String(cString: p)
            openslot_free(p)
        }
        if !heard.isEmpty {
            received += heard
            if received.count > 500_000 { received.removeFirst(received.count - 500_000) }
        }
        if modemRunning && !openslot_modem_running() {
            modemRunning = false
            status = "XARA stopped: \(log.last { $0.hasPrefix("xara: ") }.map { String($0.dropFirst(6)) } ?? "see Log")"
        }
        if partnerTyping && Date() > typingUntil { partnerTyping = false }
        // Nobody picked a slot: stay, before the engine gives up on the invitation (90 s) and the link goes silent
        if qsyInviteFrom != nil, let until = qsyInviteUntil, Date() > until {
            addSystem("No slot chosen: staying on this frequency")
            qsyInvite(slot: 0)
        }
        // The modem may still be opening its ports: try again until the deadline.
        if let d = connectDeadline, !modemConnected, Date() > d {
            connectDeadline = nil
        }
    }

    private func handle(_ e: [String: Any]) {
        let time = e["time"] as? String ?? Self.now()
        switch e["type"] as? String {
        case "modem":
            modemConnected = e["connected"] as? Bool ?? false
            if modemConnected {
                connectDeadline = nil
                status = "On the air as \(Settings().call)"
            } else if let err = e["error"] as? String {
                if let d = connectDeadline, Date() < d {
                    // Not up yet: retry shortly
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
                        guard let self, !self.modemConnected, self.connectDeadline != nil else { return }
                        self.send(["cmd": "connect_modem"])
                    }
                } else {
                    status = "Modem: \(err)"
                }
            } else {
                status = "Offline"
            }
            if !modemConnected {
                partner = nil
                slotWait = nil
            }
        case "connected":
            qsyBack = nil
            partner = e["call"] as? String
            linkBandwidth = e["bandwidth"] as? String ?? ""
            partnerInfo = ""
        case "disconnected":
            transfer = nil
            partner = nil
            partnerInfo = ""
            partnerTyping = false
            qsyInviteFrom = nil
            vmailRelayAlert = nil
        case "chat":
            chat.append(ChatLine(from: e["from"] as? String ?? "", text: e["text"] as? String ?? "", time: time,
                                 outgoing: e["outgoing"] as? Bool ?? false, system: false))
            partnerTyping = false
        case "system":
            addSystem(e["text"] as? String ?? "", time: time)
        case "typing":
            partnerTyping = true
            typingUntil = Date().addingTimeInterval(8)
        case "opening":
            var parts: [String] = []
            if let n = e["name"] as? String { parts.append(n) }
            if let q = e["qth"] as? String { parts.append(q) }
            if let l = e["locator"] as? String { parts.append(l) }
            if let snr = e["snr"] as? Int { parts.append(String(format: "%+d dB", snr)) }
            partnerInfo = parts.joined(separator: " · ")
            addSystem("de \(e["call"] as? String ?? "") \(partnerInfo)", time: time)
        case "cq":
            let call = e["call"] as? String ?? ""
            let bw = "\(e["bandwidth"] ?? "")"
            let beacon = e["beacon"] as? Bool ?? false
            var h = Heard(call: call, time: time, bandwidth: bw, beacon: beacon, count: 1)
            if let i = heard.firstIndex(where: { $0.call == call }) {
                h.count = heard.remove(at: i).count + 1
            }
            h.update(slotFrom: e)
            heard.insert(h, at: 0)
        case "broadcast":
            broadcasts.insert(BroadcastLine(from: e["from"] as? String ?? "", to: e["to"] as? String ?? "",
                                            message: e["message"] as? String ?? "", time: time), at: 0)
        case "vmail":
            addSystem("VMail from \(e["from"] as? String ?? ""): \(e["subject"] as? String ?? "")", time: time)
        case "vmail_changed":
            unread = (query(["q": "unread"]) as? Int) ?? 0
            vmailRevision += 1
        case "file_offer":
            incomingName = e["name"] as? String
            fileOffer = "\(e["name"] as? String ?? "a file") (\(e["size"] ?? 0) bytes)"
                + ((e["attachment"] as? Bool ?? false) ? ", over your incoming files size limit. Rejecting it rejects the whole VMail" : "")
        case "file_offer_closed":
            fileOffer = nil
            incomingName = nil
        case "file_incoming":
            incomingName = e["name"] as? String
        case "file_send":
            let name = e["name"] as? String ?? "file"
            switch e["state"] as? String {
            case "offered":
                transfer = FileTransfer(name: name, outgoing: true, done: 0, total: 0)
            case "sending":
                transfer = FileTransfer(name: name, outgoing: true, done: 0, total: e["total"] as? Int ?? 0, started: Date())
            default: // done, refused, failed, cancelled (the chat says which)
                if transfer?.outgoing == true { transfer = nil }
            }
        case "file_progress":
            let outgoing = e["outgoing"] as? Bool ?? false
            let done = e["done"] as? Int ?? 0, total = e["total"] as? Int ?? 0
            if outgoing {
                if transfer?.outgoing == true { transfer?.done = done; transfer?.total = total }
            } else if transfer?.outgoing != true {
                transfer = FileTransfer(name: incomingName ?? "file", outgoing: false, done: done, total: total,
                                        started: Date(), startDone: done)
            }
        case "file_saved", "file_receive_failed":
            if transfer?.outgoing == false { transfer = nil }
            incomingName = nil
        case "snr":
            if let db = e["db"] as? Double { lastSnr = Int(db.rounded()) }
        case "qsy_request":
            if let hz = e["offset_hz"] as? Int {
                qsyRequest = String(format: "QSY %+d Hz", hz)
            } else if let f = e["freq_hz"] as? Int {
                qsyRequest = "QSY to \(mhz(f)) MHz"
            }
        case "slot_wait":
            slotWait = SlotWait(slot: e["slot"] as? Int ?? 0, label: e["label"] as? String ?? "", hz: e["hz"] as? Int ?? 0,
                                cfHz: e["cf_hz"] as? Int ?? 0,
                                until: Date().addingTimeInterval(Double(e["seconds"] as? Int ?? 0)))
        case "slot_wait_end":
            slotWait = nil
        case "qsy_invite_request":
            qsyInviteOnCf = e["on_cf"] as? Bool ?? true
            qsyInviteUntil = Date().addingTimeInterval(80)
            qsyInviteFrom = e["cf_hz"] as? Int ?? Settings().frequency
        case "qsy_back":
            qsyBack = e["cf_hz"] as? Int
        case "qsy_invitation":
            qsyInvitation = QsyInvitation(targetHz: e["target_hz"] as? Int ?? 0, inviter: e["inviter"] as? Bool ?? false,
                                          seconds: e["seconds"] as? Int ?? 30)
        case "qsy_invitation_closed":
            qsyInvitation = nil
        case "vmail_relay_alert":
            vmailRelayAlert = e["call"] as? String
        case "bw_refused":
            bwRefused = true
        case "bandwidth":
            UserDefaults.standard.set(e["bw"] as? String ?? "500", forKey: Keys.bandwidth)
        case "verbose_snr":
            verboseSnr = e["on"] as? Bool ?? false
        case "verbose_snr_invite":
            verboseSnrInvite = e["call"] as? String ?? partner ?? "The station"
        case "ptt":
            ptt = e["on"] as? Bool ?? false
        case "busy":
            busy = e["on"] as? Bool ?? false
        case "log":
            addLog(e["text"] as? String ?? "")
        default:
            break
        }
    }

    private func addSystem(_ text: String, time: String = Core.now()) {
        chat.append(ChatLine(from: "", text: text, time: time, outgoing: false, system: true))
        if chat.count > 2000 { chat.removeFirst(chat.count - 2000) }
    }

    private func addLog(_ s: String) {
        log.append(s)
        if log.count > 3000 { log.removeFirst(log.count - 3000) }
    }

    nonisolated static func now() -> String {
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        f.timeZone = TimeZone(identifier: "UTC")
        return f.string(from: Date())
    }

    // MARK: audio route

    private func routeChanged() {
        route = AudioRoute.describe()
        if modemRunning, !Settings().allowBuiltIn, let why = AudioRoute.builtInReason() {
            status = "Audio moved to \(why); stopped so nothing transmits there."
            Task { await goOffline() }
        }
    }

    private func interrupted(_ type: AVAudioSession.InterruptionType?, resume: Bool) {
        switch type {
        case .began:
            resumeAfterInterruption = modemRunning
            if modemRunning { Task { await stopModem() } }
        case .ended:
            if resumeAfterInterruption && resume {
                resumeAfterInterruption = false
                Task { await goOnline() }
            }
        default:
            break
        }
    }
}

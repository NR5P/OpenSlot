import PhotosUI
import SwiftUI
import UniformTypeIdentifiers
import WebKit

@main
struct OpenslotApp: App {
    @StateObject private var core = Core()

    var body: some Scene {
        WindowGroup {
            TabView {
                ChatView().tabItem { Label("Chat", systemImage: "bubble.left.and.bubble.right") }
                StationsView().tabItem { Label("Stations", systemImage: "antenna.radiowaves.left.and.right") }
                VMailView()
                    .tabItem { Label("VMail", systemImage: "envelope") }
                    .badge(core.unread)
                ModemTabView().tabItem { Label("Modem", systemImage: "dot.radiowaves.left.and.right") }
                SettingsView().tabItem { Label("Settings", systemImage: "gearshape") }
            }
            .sheet(isPresented: $core.showCqSelector) { CqSlotSelector().environmentObject(core) }
            .sheet(isPresented: Binding(get: { core.qsyInviteFrom != nil }, set: { if !$0 { core.qsyInviteFrom = nil } })) {
                QsyInvitePicker(cf: core.qsyInviteFrom ?? Settings().frequency, onCf: core.qsyInviteOnCf).environmentObject(core)
            }
            .alert("VMail relay", isPresented: Binding(get: { core.vmailRelayAlert != nil }, set: { if !$0 { core.vmailRelayAlert = nil } })) {
                Button("Ask to QSY +750 Hz") { core.qsyRequest(750); core.vmailRelayAlert = nil }
                Button("Later", role: .cancel) { core.vmailRelayAlert = nil }
            } message: {
                Text("You have VMail waiting for \(core.vmailRelayAlert ?? "your partner"). It's relayed only off the calling frequency: invite them to QSY and it goes out automatically after the QSY.")
            }
            .alert("QSY NOW", isPresented: Binding(get: { core.qsyInvitation != nil }, set: { if !$0 { core.qsyInvitation = nil } })) {
                Button("QSY") { core.answerQsy(true) }
                Button("Reject", role: .cancel) { core.answerQsy(false) }
            } message: {
                let q = core.qsyInvitation
                Text("A QSY request to \(mhz(q?.targetHz ?? 0)) MHz has been initiated. Tap QSY and retune your radio there now. If you fail to QSY quickly, the link might be closed. Rejected automatically in \(q?.seconds ?? 30) seconds.")
            }
            // VarAC's box, with the desktop's one-tap switch to 500 Hz
            .alert("Bandwidth not allowed!", isPresented: $core.bwRefused) {
                Button("Switch to 500 Hz") { core.setBandwidth("500") }
                Button("OK", role: .cancel) {}
            } message: {
                Text("You can't use a 2300/2750Hz Bandwidth on a calling QRG as it may cause QRM to nearby VarAC stations.\nPlease note that 500Hz provides great & fast chat experience.\n\nPLEASE SWITCH YOUR BANDWIDTH TO 500HZ OR CHANGE FREQUENCY.")
            }
            .alert("QSY back", isPresented: Binding(get: { core.qsyBack != nil }, set: { if !$0 { core.qsyBack = nil } })) {
                Button("Yes, retuned") { if let hz = core.qsyBack { core.backToCf(hz) } }
                Button("No", role: .cancel) { core.qsyBack = nil }
            } message: {
                Text("Your QSO is over. Switch back to the calling frequency, \(mhz(core.qsyBack ?? 0)) MHz?")
            }
            .environmentObject(core)
        }
    }
}

/// Modem and link state, shown at the top of each screen.
struct StatusBar: View {
    @EnvironmentObject var core: Core

    var body: some View {
        VStack(spacing: 6) {
            bar
            if let w = core.slotWait { SlotWaitBanner(wait: w) }
        }
        .padding(.horizontal)
        .padding(.vertical, 6)
        .background(.bar)
    }

    private var bar: some View {
        HStack(spacing: 8) {
            Circle().fill(core.modemConnected ? .green : .gray).frame(width: 10, height: 10)
            Text(core.partner.map { "In QSO with \($0)" } ?? core.status)
                .font(.footnote).lineLimit(1)
            Spacer()
            if core.ptt {
                Text("TX").font(.caption.bold()).padding(.horizontal, 6).background(.red, in: Capsule()).foregroundStyle(.white)
            } else if core.busy {
                Text("BUSY").font(.caption.bold()).padding(.horizontal, 6).background(.orange, in: Capsule()).foregroundStyle(.white)
            }
            OnlineButton()
        }
    }
}

/// Standing by on our slot after a CQ (VarAC), with the countdown back to the calling frequency.
struct SlotWaitBanner: View {
    @EnvironmentObject var core: Core
    let wait: SlotWait

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Waiting on \(wait.label) · \(mhz(wait.hz)) MHz").font(.footnote.bold())
                    Text("Back to the calling frequency in \(max(0, Int(wait.until.timeIntervalSince(ctx.date)))) s")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Return now") { core.returnToCf() }.font(.footnote).buttonStyle(.bordered)
            }
            .padding(8)
            .background(Color.orange.opacity(0.18), in: RoundedRectangle(cornerRadius: 8))
        }
    }
}

/// The VarAC slots around the calling frequency, each with its frequency; tap to choose.
struct SlotRows: View {
    @Binding var slot: Int
    let cf: Int

    var body: some View {
        ForEach(Slots.byFrequency, id: \.self) { s in
            Button {
                slot = s
            } label: {
                HStack {
                    Text(s == 0 ? "CF" : "Slot \(s)")
                    Text(s == 0 ? "calling frequency" : String(format: "%+d Hz", Slots.offset(s)))
                        .font(.caption).foregroundStyle(.secondary)
                    Spacer()
                    Text("\(mhz(cf + Slots.offset(s))) MHz").monospacedDigit().foregroundStyle(.secondary)
                    if s == slot { Image(systemName: "checkmark").foregroundStyle(.tint) }
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }
}

/// VarAC's QSY invitation in the opening: we called, the partner allows auto-QSY, so we
/// choose the slot both stations move to (the phone doesn't tune: retune by hand).
struct QsyInvitePicker: View {
    @EnvironmentObject var core: Core
    let cf: Int
    let onCf: Bool
    @State private var slot = Settings().cqSlot

    var body: some View {
        NavigationStack {
            List {
                Section {
                    SlotRows(slot: $slot, cf: cf)
                } header: {
                    TimelineView(.periodic(from: .now, by: 1)) { ctx in
                        Text("QSY to · staying automatically in \(max(0, Int(core.qsyInviteUntil?.timeIntervalSince(ctx.date) ?? 0))) s")
                    }
                } footer: {
                    Text(slot == 0
                         ? (onCf ? "Slot 0 stays on the calling frequency; links left there are closed after 4 minutes." : "Slot 0 stays where you are.")
                         : "Both stations move to \(mhz(cf + Slots.offset(slot))) MHz. Retune your radio when the QSY is agreed.")
                }
            }
            .navigationTitle("QSY invitation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Stay") { core.qsyInvite(slot: 0) } }
                ToolbarItem(placement: .confirmationAction) { Button("Invite") { core.qsyInvite(slot: slot) } }
            }
        }
        .presentationDetents([.medium, .large])
        .interactiveDismissDisabled()
    }
}

/// VarAC's CQ slot selector: pick the slot you'll wait on after the CQ, then call.
struct CqSlotSelector: View {
    @EnvironmentObject var core: Core
    @Environment(\.dismiss) private var dismiss
    @State private var slot = Settings().cqSlot
    private let cf = Settings().frequency

    var body: some View {
        NavigationStack {
            List {
                Section {
                    SlotRows(slot: $slot, cf: cf)
                } header: { Text("Wait on") } footer: {
                    if slot == 0 {
                        Text("Slot 0 is the calling frequency itself. QSOs on VarAC calling frequencies are prohibited.")
                            .foregroundStyle(.orange)
                    } else {
                        Text("There's one calling frequency per band. Your CQ names a slot around it where you'll wait for replies. The phone doesn't tune the radio: after the CQ, tune it to \(mhz(cf + Slots.offset(slot))) MHz. Make sure the slot is in the band and free.")
                    }
                }
            }
            .navigationTitle("Call CQ")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Call CQ") {
                        core.callCq(slot: slot)
                        dismiss()
                    }
                }
            }
        }
        .presentationDetents([.medium, .large])
    }
}

struct OnlineButton: View {
    @EnvironmentObject var core: Core

    var body: some View {
        if core.modemConnected || core.modemRunning {
            Button("Go offline") { Task { await core.goOffline() } }.font(.footnote)
        } else {
            Button("Go on air") { Task { await core.goOnline() } }.font(.footnote.bold())
        }
    }
}

// MARK: frequency and bandwidth

/// A small colored label (band, calling frequency).
struct Pill: View {
    let text: String
    let color: Color

    var body: some View {
        Text(text).font(.caption.bold()).padding(.horizontal, 6).padding(.vertical, 1)
            .background(color, in: Capsule()).foregroundStyle(.white)
    }
}

/// The dial frequency (tap it for the picker) and VARA HF's bandwidth buttons, as on the
/// desktop's main screen. The phone doesn't tune the radio: picking a frequency logs it and
/// applies the calling-frequency rules there.
struct FrequencyBar: View {
    @EnvironmentObject var core: Core
    @AppStorage(Keys.frequency) private var frequency = 14_105_000
    @AppStorage(Keys.bandwidth) private var bandwidth = "500"
    @State private var picking = false

    var body: some View {
        HStack(spacing: 6) {
            Button { picking = true } label: {
                HStack(spacing: 2) {
                    Text(dial(frequency)).font(.system(size: 17, weight: .semibold, design: .monospaced)).fixedSize()
                    Image(systemName: "chevron.down").font(.caption2.bold())
                }
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Frequency \(mhz(frequency)) MHz, change")
            if let band = core.bandOf(frequency) { Pill(text: band, color: .accentColor) }
            if core.isCallingFrequency(frequency) { Pill(text: "CF", color: .orange) }
            Spacer(minLength: 4)
            Picker("Bandwidth", selection: Binding(get: { bandwidth }, set: { core.setBandwidth($0) })) {
                Text("500").tag("500")
                Text("2300").tag("2300")
                Text("2750").tag("2750")
            }
            .pickerStyle(.segmented)
            .controlSize(.small)
            .fixedSize()
        }
        .padding(.horizontal).padding(.vertical, 4)
        .sheet(isPresented: $picking) { FrequencyPicker(current: frequency).environmentObject(core) }
    }
}

/// The desktop's frequency picker: type any frequency, a VarAC calling frequency, or one of your own.
struct FrequencyPicker: View {
    @EnvironmentObject var core: Core
    @Environment(\.dismiss) private var dismiss
    let current: Int
    @State private var entry = ""
    @State private var error: String?
    @State private var mine: [SavedFrequency] = []
    @State private var name = ""

    private func pick(_ hz: Int) {
        if hz != current { core.setFrequency(hz) }
        dismiss()
    }

    private func tune() {
        if let hz = core.parseFrequency(entry) {
            pick(hz)
        } else {
            error = "Not a frequency: \(entry.trimmingCharacters(in: .whitespaces))"
        }
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack {
                        TextField("14.1055 or 14105.5", text: $entry)
                            .keyboardType(.decimalPad)
                            .onChange(of: entry) { error = nil }
                        Button("Tune", action: tune)
                            .buttonStyle(.borderless)
                            .disabled(entry.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                } header: { Text("Enter a frequency") } footer: {
                    Text(error ?? "MHz below 1000, kHz below 1,000,000, else Hz")
                        .foregroundStyle(error == nil ? Color.secondary : Color.red)
                }
                Section("VarAC calling frequencies") {
                    LazyVGrid(columns: [GridItem(.flexible(), alignment: .leading), GridItem(.flexible(), alignment: .leading)], spacing: 4) {
                        ForEach(core.callingFrequencies, id: \.self) { cf in
                            choice("\(cf.band) \(mhz(cf.hz))", selected: cf.hz == current) { pick(cf.hz) }
                        }
                    }
                }
                Section("My frequencies") {
                    if mine.isEmpty {
                        Text("None yet: add the frequency you're on below.").font(.footnote).foregroundStyle(.secondary)
                    }
                    ForEach(mine, id: \.self) { f in
                        Button { pick(f.hz) } label: {
                            HStack {
                                Text(mhz(f.hz)).monospacedDigit()
                                Text(core.bandOf(f.hz) ?? "").foregroundStyle(.secondary)
                                Text(f.name)
                                Spacer()
                                if f.hz == current { Image(systemName: "checkmark").foregroundStyle(.tint) }
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    .onDelete { offsets in
                        mine.remove(atOffsets: offsets)
                        core.myFrequencies = mine
                    }
                    HStack {
                        TextField("Name (optional)", text: $name)
                        Button("Add \(mhz(current))") {
                            mine = (mine + [SavedFrequency(hz: current, name: name.trimmingCharacters(in: .whitespaces))]).sorted { $0.hz < $1.hz }
                            core.myFrequencies = mine
                            name = ""
                        }
                        .buttonStyle(.borderless)
                        .disabled(mine.contains { $0.hz == current })
                    }
                }
            }
            .navigationTitle("Frequency")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } }
            }
            .onAppear { mine = core.myFrequencies }
        }
        .presentationDetents([.medium, .large])
    }

    private func choice(_ label: String, selected: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.callout.monospaced())
                .fontWeight(selected ? .bold : .regular)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 6).padding(.horizontal, 4)
                .background(selected ? Color.accentColor.opacity(0.18) : .clear, in: RoundedRectangle(cornerRadius: 6))
                .contentShape(Rectangle())
        }
        .buttonStyle(.borderless)
        .foregroundStyle(.primary)
    }
}

// MARK: chat

struct ChatView: View {
    @EnvironmentObject var core: Core
    @State private var text = ""
    @State private var callTo = ""
    @State private var askLastHeard = false
    @State private var lastHeardCall = ""
    @State private var pickingFile = false
    @State private var pickingPhoto = false
    @State private var photo: PhotosPickerItem?
    @State private var outgoing: OutgoingFile?
    @State private var pickError: String?
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                StatusBar()
                FrequencyBar()
                if let p = core.partner {
                    HStack {
                        VStack(alignment: .leading) {
                            Text(p).font(.headline)
                            if !core.partnerInfo.isEmpty { Text(core.partnerInfo).font(.caption).foregroundStyle(.secondary) }
                        }
                        Spacer()
                        Menu {
                            Toggle("Verbose SNR", isOn: Binding(get: { core.verboseSnr }, set: { core.setVerboseSnr($0) }))
                            Button("Request SNR") { core.send(["cmd": "snr_request"]) }
                            Menu("Inquire") {
                                Button("Station info") { core.inquire("info") }
                                Button("Last heard (all)") { core.inquire("last_heard") }
                                Button("Last heard a station…") { askLastHeard = true }
                                Button("Parked VMail status") { core.inquire("parked") }
                                Button("Locator") { core.inquire("locator") }
                                Button("GPS position") { core.inquire("gps") }
                                Button("Version") { core.inquire("version") }
                                Button("Frequency schedule") { core.inquire("schedule") }
                            }
                            Menu("Ask to QSY") {
                                ForEach([750, 1500, 2250, -750, -1500, -2250], id: \.self) { hz in
                                    Button(String(format: "%+d Hz", hz)) { core.qsyRequest(hz) }
                                }
                            }
                        } label: { Image(systemName: "ellipsis.circle").imageScale(.large) }
                        Button("Disconnect", role: .destructive) { core.disconnect() }.buttonStyle(.bordered)
                    }
                    .padding(.horizontal).padding(.vertical, 6)
                } else {
                    HStack {
                        TextField("Call sign", text: $callTo)
                            .textInputAutocapitalization(.characters).autocorrectionDisabled()
                            .textFieldStyle(.roundedBorder)
                        Button("Connect") { core.connect(callTo) }.buttonStyle(.borderedProminent)
                            .disabled(callTo.isEmpty || !core.modemConnected)
                        Menu {
                            Button("Ping") { core.ping(callTo) }.disabled(callTo.isEmpty)
                            Button("Call CQ") { core.cq() }
                            Button("Send beacon") { core.beacon() }
                            Button("Abort", role: .destructive) { core.abort() }
                        } label: { Image(systemName: "ellipsis.circle").imageScale(.large) }
                            .disabled(!core.modemConnected)
                    }
                    .padding(.horizontal).padding(.vertical, 6)
                }
                Divider()
                ScrollViewReader { proxy in
                    List(core.chat) { line in
                        ChatRow(line: line).id(line.id).listRowSeparator(.hidden)
                    }
                    .listStyle(.plain)
                    .scrollDismissesKeyboard(.immediately)
                    .onChange(of: core.chat.count) {
                        if let last = core.chat.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
                    }
                }
                if core.partnerTyping, let p = core.partner {
                    Text("\(p) is typing…").font(.caption).foregroundStyle(.secondary).padding(.bottom, 2)
                }
                if let t = core.transfer { TransferBar(transfer: t) }
                HStack {
                    Menu {
                        Button { pickingFile = true } label: { Label("File", systemImage: "doc") }
                        Button { pickingPhoto = true } label: { Label("Photo", systemImage: "photo") }
                    } label: { Image(systemName: "paperclip").imageScale(.large) }
                        .disabled(core.partner == nil || core.transfer != nil)
                    TextField(core.partner == nil ? "Not in a QSO" : "Message", text: $text, axis: .vertical)
                        .lineLimit(1...4).textFieldStyle(.roundedBorder).focused($focused)
                        .onChange(of: text) { old, new in
                            if old.isEmpty && !new.isEmpty && core.partner != nil { core.send(["cmd": "typing"]) }
                        }
                    Button {
                        let t = text.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !t.isEmpty else { return }
                        core.sendChat(t)
                        text = ""
                    } label: { Image(systemName: "paperplane.fill") }
                        .buttonStyle(.borderedProminent)
                        .disabled(core.partner == nil || text.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .padding()
            }
            .navigationTitle("Chat")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Clear") { core.clearChat() }.disabled(core.chat.isEmpty)
                }
            }
            .fileImporter(isPresented: $pickingFile, allowedContentTypes: [.item]) { result in
                switch result {
                case .success(let url):
                    do { outgoing = OutgoingFile(path: try OutgoingFile.copy(url)) } catch { pickError = error.localizedDescription }
                case .failure(let e):
                    pickError = e.localizedDescription
                }
            }
            .photosPicker(isPresented: $pickingPhoto, selection: $photo, matching: .images)
            .onChange(of: photo) {
                guard let item = photo else { return }
                photo = nil
                Task {
                    do { outgoing = OutgoingFile(path: try await OutgoingFile.copy(item)) } catch { pickError = error.localizedDescription }
                }
            }
            .sheet(item: $outgoing) { f in SendFileSheet(path: f.path).environmentObject(core) }
            .alert("Can't send that", isPresented: Binding(get: { pickError != nil }, set: { if !$0 { pickError = nil } })) {
                Button("OK", role: .cancel) {}
            } message: { Text(pickError ?? "") }
            .alert("File offered", isPresented: Binding(get: { core.fileOffer != nil }, set: { if !$0 { core.fileOffer = nil } })) {
                Button("Receive") { core.send(["cmd": "file_accept"]); core.fileOffer = nil }
                Button("Decline", role: .cancel) { core.send(["cmd": "file_decline"]); core.fileOffer = nil }
            } message: {
                Text("\(core.partner ?? "The station") offers \(core.fileOffer ?? ""). It is saved in the Files app under Openslot › Received.")
            }
            .alert("Last heard", isPresented: $askLastHeard) {
                TextField("Call sign", text: $lastHeardCall).textInputAutocapitalization(.characters).autocorrectionDisabled()
                Button("Ask") { core.inquire("last_heard_call", call: lastHeardCall); lastHeardCall = "" }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("When did \(core.partner ?? "your partner") last hear this station?")
            }
            .alert("Verbose SNR", isPresented: Binding(get: { core.verboseSnrInvite != nil }, set: { if !$0 { core.verboseSnrInvite = nil } })) {
                Button("Accept") { core.answerVerboseSnr(true) }
                Button("Reject", role: .cancel) { core.answerVerboseSnr(false) }
            } message: {
                Text("\(core.verboseSnrInvite ?? "The station") invites you to exchange SNR reports every minute.")
            }
            .alert("Frequency change", isPresented: Binding(get: { core.qsyRequest != nil }, set: { if !$0 { core.qsyRequest = nil } })) {
                Button("Accept") { core.send(["cmd": "qsy_accept"]); core.qsyRequest = nil }
                Button("Reject", role: .cancel) { core.send(["cmd": "qsy_reject"]); core.qsyRequest = nil }
            } message: {
                Text("\(core.partner ?? "The station") asks to \(core.qsyRequest ?? ""). Retune your radio if you accept.")
            }
        }
    }
}

/// A file picked to send: a copy in the app's own temporary folder, under its own name.
struct OutgoingFile: Identifiable {
    let path: String
    var id: String { path }

    struct PickError: LocalizedError {
        let errorDescription: String?
    }

    private static func folder() throws -> URL {
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("Outgoing/\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    /// A file from the Files app (outside the app's sandbox until we're let in).
    static func copy(_ url: URL) throws -> String {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let dest = try folder().appendingPathComponent(url.lastPathComponent)
        try FileManager.default.copyItem(at: url, to: dest)
        return dest.path
    }

    /// A photo from the library. HEIC (what the camera saves) becomes JPEG, which other
    /// stations can open and the shrinker can read; the camera's orientation is kept.
    static func copy(_ item: PhotosPickerItem) async throws -> String {
        guard let data = try await item.loadTransferable(type: Data.self) else {
            throw PickError(errorDescription: "The photo couldn't be read.")
        }
        let type = item.supportedContentTypes.first
        let f = DateFormatter()
        f.dateFormat = "yyyyMMdd_HHmmss"
        let stamp = f.string(from: Date())
        let name: String, bytes: Data
        if let type, type.conforms(to: .jpeg) || type.conforms(to: .png) || type.conforms(to: .gif) || type.conforms(to: .bmp) {
            name = "IMG_\(stamp).\(type.preferredFilenameExtension ?? "jpg")"
            bytes = data
        } else if let jpg = UIImage(data: data)?.jpegData(compressionQuality: 0.9) {
            name = "IMG_\(stamp).jpg"
            bytes = jpg
        } else {
            throw PickError(errorDescription: "This kind of photo can't be sent.")
        }
        let dest = try folder().appendingPathComponent(name)
        try bytes.write(to: dest)
        return dest.path
    }
}

/// The file going over the link, above the message box: waiting for the partner's
/// answer, then how much is sent (or received); ours can be stopped.
struct TransferBar: View {
    @EnvironmentObject var core: Core
    let transfer: FileTransfer
    @State private var confirmStop = false

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: transfer.outgoing ? "arrow.up.doc" : "arrow.down.doc").foregroundStyle(.tint)
            VStack(alignment: .leading, spacing: 3) {
                Text(transfer.name).font(.subheadline).lineLimit(1).truncationMode(.middle)
                if transfer.total == 0 {
                    Text(transfer.outgoing ? "Waiting for \(core.partner ?? "the station") to accept…" : "Receiving…")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    ProgressView(value: transfer.fraction)
                    Text("\(transfer.outgoing ? "Sent" : "Received") \(Int(transfer.fraction * 100))% of \(byteText(transfer.total))"
                         + (transfer.timeLeft.map { " · \(spanText($0)) left" } ?? ""))
                        .font(.caption).foregroundStyle(.secondary).monospacedDigit()
                }
            }
            if transfer.outgoing {
                // Withdrawing an offer is free; once the data is going only dropping the link stops it
                Button("Stop", role: .destructive) { if transfer.total == 0 { core.cancelFile() } else { confirmStop = true } }
                    .buttonStyle(.bordered).controlSize(.small)
            }
        }
        .padding(.horizontal).padding(.vertical, 8)
        .background(.bar)
        .alert("Stop sending?", isPresented: $confirmStop) {
            Button("Stop and end the QSO", role: .destructive) { core.cancelFile() }
            Button("Keep sending", role: .cancel) {}
        } message: {
            Text("The modem can't take back what it's already sending, so stopping ends the QSO with \(core.partner ?? "the station").")
        }
    }
}

/// "about 3 min" / "40 s".
func spanText(_ secs: TimeInterval) -> String {
    secs < 60 ? "\(max(5, Int((secs / 5).rounded()) * 5)) s" : "about \(Int((secs / 60).rounded())) min"
}

func byteText(_ n: Int) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(n), countStyle: .file)
}

/// Before a file goes: its size, whether VARA allows it on this link and how long it
/// takes. A picture goes through VarAC's image shrinker (resolution and quality), set at
/// first to the largest size that fits, or as it is.
struct SendFileSheet: View {
    @EnvironmentObject var core: Core
    @Environment(\.dismiss) private var dismiss
    let path: String
    @State private var info: [String: Any]?
    @State private var shrink = true
    @State private var pct = 100.0
    @State private var quality = 50.0
    @State private var working = false

    private var isImage: Bool { info?["image"] as? Bool ?? false }
    private var shrunk: [String: Any]? { info?["shrunk"] as? [String: Any] }
    /// What goes out: the shrinker's picture, or the file as it is
    private var chosen: [String: Any]? { isImage && shrink ? shrunk : info }
    private var fits: Bool { chosen?["fits"] as? Bool ?? false }

    var body: some View {
        NavigationStack {
            Form {
                if let error = info?["error"] as? String {
                    Text(error).foregroundStyle(.red)
                } else if let info {
                    Section {
                        LabeledContent("File", value: info["name"] as? String ?? "")
                        LabeledContent("Size", value: sizeLine(info))
                        if isImage, let w = info["w"] as? Int, let h = info["h"] as? Int {
                            LabeledContent("Picture", value: "\(w) × \(h)")
                        }
                        if !(isImage && shrink) { estimateRow(info) }
                    }
                    if isImage {
                        Section {
                            Toggle("Shrink the picture", isOn: $shrink)
                            if shrink {
                                VStack(alignment: .leading) {
                                    Text("Resolution \(Int(pct)) %")
                                    Slider(value: $pct, in: 1...100, step: 1) { editing in if !editing { reload() } }
                                }
                                VStack(alignment: .leading) {
                                    Text("Quality \(Int(quality))")
                                    Slider(value: $quality, in: 0...50, step: 2) { editing in if !editing { reload() } }
                                }
                                if let s = shrunk {
                                    LabeledContent("Picture", value: "\(s["w"] as? Int ?? 0) × \(s["h"] as? Int ?? 0)")
                                    LabeledContent("Size", value: sizeLine(s))
                                    estimateRow(s)
                                } else if let e = info["shrink_error"] as? String {
                                    Text(e).foregroundStyle(.red)
                                }
                            }
                        } footer: {
                            Text("As VarAC's image shrinker: the picture is scaled and sent as a JPEG.")
                        }
                    }
                    if chosen != nil && !fits, let limit = info["limit"] as? Int {
                        Text("VARA HF doesn't allow files over \(byteText(limit)) compressed at \(core.fileBandwidth) Hz."
                             + (isImage ? " Shrink the picture more." : ""))
                            .foregroundStyle(.red)
                    }
                } else {
                    HStack { ProgressView(); Text("Reading the file…").foregroundStyle(.secondary) }
                }
            }
            .overlay { if working && info != nil { ProgressView().padding().background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10)) } }
            .navigationTitle("Send a file")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send") {
                        core.sendFile(path: path, pct: isImage && shrink ? Int(pct) : nil, quality: Int(quality))
                        dismiss()
                    }
                    .disabled(working || chosen == nil || !fits || core.partner == nil)
                }
            }
            .task { await load(pct: nil) }
        }
    }

    private func sizeLine(_ d: [String: Any]) -> String {
        "\(byteText(d["bytes"] as? Int ?? 0)), \(byteText(d["compressed"] as? Int ?? 0)) compressed"
    }

    @ViewBuilder private func estimateRow(_ d: [String: Any]) -> some View {
        LabeledContent("Time", value: d["estimate"] as? String ?? "")
    }

    private func reload() { Task { await load(pct: Int(pct)) } }

    private func load(pct asked: Int?) async {
        working = true
        let i = await Core.fileInfo(path: path, bandwidth: core.fileBandwidth, snr: core.lastSnr, pct: asked,
                                    quality: Int(quality), minutes: Settings().photoMinutes)
        if asked == nil, let s = i["shrunk"] as? [String: Any] {
            pct = Double(s["pct"] as? Int ?? 100)
            // a picture that fits as it is goes as it is unless the operator shrinks it
            shrink = !(i["fits"] as? Bool ?? false)
        }
        info = i
        working = false
    }
}

struct ChatRow: View {
    let line: ChatLine

    var body: some View {
        if line.system {
            Text("\(line.time)  \(line.text)")
                .font(.caption).foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .center)
                .multilineTextAlignment(.center)
        } else {
            HStack {
                if line.outgoing { Spacer(minLength: 40) }
                VStack(alignment: line.outgoing ? .trailing : .leading, spacing: 2) {
                    Text("\(line.from)  \(line.time)").font(.caption2).foregroundStyle(.secondary)
                    Text(line.text)
                        .padding(.horizontal, 10).padding(.vertical, 6)
                        .background(line.outgoing ? Color.accentColor.opacity(0.85) : Color.secondary.opacity(0.18),
                                    in: RoundedRectangle(cornerRadius: 12))
                        .foregroundStyle(line.outgoing ? .white : .primary)
                        .textSelection(.enabled)
                }
                if !line.outgoing { Spacer(minLength: 40) }
            }
        }
    }
}

// MARK: stations

struct StationsView: View {
    @EnvironmentObject var core: Core
    @State private var broadcastText = ""
    @State private var composing = false
    @State private var slotCall: Heard?

    /// VarAC: a CQ naming a slot is answered there, so ask the operator to tune first.
    private func answer(_ h: Heard) {
        if h.slot != 0 && h.slotHz != nil && !h.beacon { slotCall = h } else { core.connect(h.call) }
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                StatusBar()
                List {
                    Section("Heard (CQ and beacons)") {
                        if core.heard.isEmpty {
                            Text("Listening… stations calling CQ or beaconing show up here.").foregroundStyle(.secondary)
                        }
                        ForEach(core.heard) { h in
                            HStack {
                                VStack(alignment: .leading) {
                                    Text(h.call).font(.headline)
                                    Text("\(h.beacon ? "Beacon" : "CQ") · \(h.bandwidth) Hz").font(.caption).foregroundStyle(.secondary)
                                    if h.slot != 0 && !h.beacon {
                                        Text("Waits on \(h.slotLabel)").font(.caption).foregroundStyle(.orange)
                                    }
                                }
                                Spacer()
                                Text(h.time).font(.caption).foregroundStyle(.secondary)
                            }
                            .contentShape(Rectangle())
                            .contextMenu {
                                Button("Connect") { answer(h) }
                                Button("Ping") { core.ping(h.call) }
                            }
                            .swipeActions {
                                Button("Connect") { answer(h) }.tint(.accentColor)
                                Button("Ping") { core.ping(h.call) }.tint(.orange)
                            }
                        }
                    }
                    Section("Broadcasts") {
                        if core.broadcasts.isEmpty { Text("No broadcasts heard yet").foregroundStyle(.secondary) }
                        ForEach(core.broadcasts) { b in
                            VStack(alignment: .leading) {
                                Text("\(b.from) → \(b.to)  \(b.time)").font(.caption).foregroundStyle(.secondary)
                                Text(b.message)
                            }
                        }
                    }
                }
            }
            .navigationTitle("Stations")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button("Call CQ") { core.cq() }
                        Button("Send beacon") { core.beacon() }
                        Button("Broadcast…") { composing = true }
                    } label: { Image(systemName: "megaphone") }
                        .disabled(!core.modemConnected)
                }
            }
            .alert("Answer on the slot", isPresented: Binding(get: { slotCall != nil }, set: { if !$0 { slotCall = nil } })) {
                Button("Tuned, connect") { if let h = slotCall { core.connectOnSlot(h) }; slotCall = nil }
                Button("Cancel", role: .cancel) { slotCall = nil }
            } message: {
                if let h = slotCall, let hz = h.slotHz {
                    Text("\(h.call) called CQ at \(h.time) and waits on \(h.slotLabel). Tune your radio to \(mhz(hz)) MHz (USB), then connect. Retune to the calling frequency after the QSO.")
                }
            }
            .alert("Broadcast", isPresented: $composing) {
                TextField("Message (81 characters)", text: $broadcastText)
                Button("Send") {
                    core.send(["cmd": "broadcast", "text": String(broadcastText.prefix(81))])
                    broadcastText = ""
                }
                Button("Cancel", role: .cancel) {}
            }
        }
    }
}

// MARK: VMail

struct VMailView: View {
    @EnvironmentObject var core: Core
    @State private var folder = "inbox"
    @State private var items: [VMailItem] = []
    @State private var composing = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                StatusBar()
                Picker("Folder", selection: $folder) {
                    Text("Inbox").tag("inbox")
                    Text("Outbox").tag("outbox")
                    Text("Sent").tag("sent")
                    Text("Parked").tag("parking")
                }
                .pickerStyle(.segmented)
                .padding()
                List {
                    if items.isEmpty { Text("No VMail here").foregroundStyle(.secondary) }
                    ForEach(items) { m in
                        NavigationLink(value: m) {
                            VStack(alignment: .leading, spacing: 2) {
                                HStack {
                                    Text(folder == "inbox" || folder == "parking" ? m.from : m.to)
                                        .font(m.read ? .body : .body.bold())
                                    if m.urgent { Image(systemName: "exclamationmark.circle.fill").foregroundStyle(.red) }
                                    Spacer()
                                    Text(m.time).font(.caption).foregroundStyle(.secondary)
                                }
                                Text(m.subject).font(.subheadline).lineLimit(1)
                            }
                        }
                    }
                    .onDelete { idx in
                        idx.map { items[$0].id }.forEach(core.delete)
                    }
                }
                .listStyle(.plain)
            }
            .navigationTitle("VMail")
            .navigationBarTitleDisplayMode(.inline)
            .navigationDestination(for: VMailItem.self) { m in
                VMailDetail(item: m, folder: folder)
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { composing = true } label: { Image(systemName: "square.and.pencil") }
                }
            }
            .sheet(isPresented: $composing) { ComposeView(to: core.partner ?? "") }
            .onAppear(perform: reload)
            .onChange(of: folder) { reload() }
            .onChange(of: core.vmailRevision) { reload() }
        }
    }

    private func reload() { items = core.vmails(folder) }
}

struct VMailDetail: View {
    @EnvironmentObject var core: Core
    let item: VMailItem
    let folder: String
    @State private var replying = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                Text(item.subject).font(.title3.bold())
                Text("From \(item.from) to \(item.to)").font(.subheadline)
                Text(item.time).font(.caption).foregroundStyle(.secondary)
                Divider()
                Text(item.body).textSelection(.enabled)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding()
        }
        .toolbar {
            if folder == "inbox" {
                Button("Reply") { replying = true }
            }
        }
        .sheet(isPresented: $replying) {
            ComposeView(to: item.from, subject: item.subject.hasPrefix("Re:") ? item.subject : "Re: \(item.subject)")
        }
        .onAppear { if !item.read { core.markRead(item.id) } }
    }
}

struct ComposeView: View {
    @EnvironmentObject var core: Core
    @Environment(\.dismiss) private var dismiss
    @State var to: String
    @State var subject = ""
    @State private var body_ = ""
    @State private var urgent = false

    var body: some View {
        NavigationStack {
            Form {
                TextField("To (call sign)", text: $to).textInputAutocapitalization(.characters).autocorrectionDisabled()
                TextField("Subject", text: $subject)
                Toggle("Urgent", isOn: $urgent)
                Section("Message") {
                    TextField("Message", text: $body_, axis: .vertical).lineLimit(6...20)
                }
                Section {
                    Text("It goes out now if you're in a QSO with \(to.isEmpty ? "them" : to.uppercased()); otherwise it waits in the Outbox for your next QSO with them.")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
            .navigationTitle("New VMail")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send") {
                        core.sendVMail(to: to, subject: subject, body: body_, urgent: urgent)
                        dismiss()
                    }
                    .disabled(to.trimmingCharacters(in: .whitespaces).isEmpty || body_.isEmpty)
                }
            }
        }
    }
}

// MARK: settings

struct SettingsView: View {
    @EnvironmentObject var core: Core
    @AppStorage(Keys.call) private var call = ""
    @AppStorage(Keys.name) private var name = ""
    @AppStorage(Keys.qth) private var qth = ""
    @AppStorage(Keys.locator) private var locator = ""
    @AppStorage(Keys.frequency) private var frequency = 14_105_000
    @AppStorage(Keys.allowInfo) private var allowInfo = true
    @AppStorage(Keys.allowPings) private var allowPings = true
    @AppStorage(Keys.verboseSnrAuto) private var verboseSnrAuto = false
    @AppStorage(Keys.cqSlotWait) private var cqSlotWait = 300
    @AppStorage(Keys.skipSlotSelector) private var skipSlotSelector = false
    @AppStorage(Keys.email) private var email = ""
    @AppStorage(Keys.fullCall) private var fullCall = ""
    @AppStorage(Keys.welcome) private var welcome = ""
    @AppStorage(Keys.welcomeAlways) private var welcomeAlways = false
    @AppStorage(Keys.callIdMinutes) private var callIdMinutes = 10
    @AppStorage(Keys.idleMinutes) private var idleMinutes = 5
    @AppStorage(Keys.allowLastHeard) private var allowLastHeard = true
    @AppStorage(Keys.multiBandLastHeard) private var multiBandLastHeard = false
    @AppStorage(Keys.photoMinutes) private var photoMinutes = 5

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Call sign", text: $call).textInputAutocapitalization(.characters).autocorrectionDisabled()
                    TextField("Name", text: $name)
                    TextField("QTH", text: $qth)
                    TextField("Grid locator", text: $locator).textInputAutocapitalization(.characters).autocorrectionDisabled()
                } header: { Text("Station") } footer: {
                    Text("Sent in your opening report. Changes apply the next time you go on the air.")
                }

                Section {
                    TextField("Email (shared in INFO)", text: $email)
                        .textInputAutocapitalization(.never).autocorrectionDisabled().keyboardType(.emailAddress)
                    TextField("Complex call sign, e.g. W/4Z1AC/QRP", text: $fullCall)
                        .textInputAutocapitalization(.characters).autocorrectionDisabled()
                } header: { Text("Profile") } footer: {
                    Text("Leave the email empty to keep it private. A complex call sign is sent in your opening (<FC:>); VARA links with the plain one.")
                }
                .onChange(of: email) { core.applyPolicy() }
                .onChange(of: fullCall) { core.applyPolicy() }

                Section("Modem") {
                    Text("The modem (XARA on this phone or VARA on the network), its PTT and drive, and the Registered (VARA licence) switch are on the Modem tab.")
                        .font(.footnote).foregroundStyle(.secondary)
                }

                Section {
                    Toggle("Answer info requests", isOn: $allowInfo)
                    Toggle("Answer pings", isOn: $allowPings)
                    Toggle("Auto accept verbose SNR", isOn: $verboseSnrAuto)
                    Toggle("Allow last heard peeking", isOn: $allowLastHeard)
                    Toggle("Last heard on all bands", isOn: $multiBandLastHeard)
                    TextField("Welcome message", text: $welcome, axis: .vertical).lineLimit(1...3)
                    Toggle("Send the welcome even when away", isOn: $welcomeAlways)
                    Stepper("Call ID every \(callIdMinutes) min", value: $callIdMinutes, in: 5...30)
                    Stepper(idleMinutes == 0 ? "Never disconnect when idle" : "Disconnect after \(idleMinutes) min idle",
                            value: $idleMinutes, in: 0...60)
                } header: { Text("QSO") } footer: {
                    Text("With info requests off they're refused; with pings off the modem doesn't answer them. The welcome goes to stations that call you (<LOC> is your locator).")
                }
                .onChange(of: allowLastHeard) { core.applyPolicy() }
                .onChange(of: multiBandLastHeard) { core.applyPolicy() }
                .onChange(of: welcome) { core.applyPolicy() }
                .onChange(of: welcomeAlways) { core.applyPolicy() }
                .onChange(of: callIdMinutes) { core.applyPolicy() }
                .onChange(of: idleMinutes) { core.applyPolicy() }
                .onChange(of: allowInfo) { core.applyPolicy() }
                .onChange(of: allowPings) { core.applyPolicy() }
                .onChange(of: verboseSnrAuto) { core.applyPolicy() }

                Section {
                    Toggle("Skip slot selector", isOn: $skipSlotSelector)
                    Stepper("Wait on the slot \(cqSlotWait) s", value: $cqSlotWait, in: 30...900, step: 30)
                } header: { Text("CQ") } footer: {
                    Text("A CQ names the slot you'll wait on for replies, then you go back to the calling frequency. With the selector skipped, CQs name no slot.")
                }

                Section {
                    Picker("Photos: send in", selection: $photoMinutes) {
                        ForEach([2, 5, 10, 20, 0], id: \.self) { m in
                            Text(m == 0 ? "Largest that fits" : "About \(m) min").tag(m)
                        }
                    }
                } header: { Text("Files") } footer: {
                    Text("A photo starts at the size that goes out in about this time (at the link's SNR), never over what VARA allows. You can still change it before sending.")
                }

                Section {
                    TextField("Dial frequency (Hz)", value: $frequency, format: .number.grouping(.never)).keyboardType(.numberPad)
                        .onSubmit { core.send(["cmd": "set_frequency", "hz": frequency]) }
                } header: { Text("Radio") } footer: {
                    Text("For the QSO log (also on the Chat screen: tap the frequency); set your radio to it (USB). The phone doesn't tune the radio. On a calling frequency only 500 Hz is allowed for CQ, connect, ping and beacon.")
                }

                Section {
                    Text(core.status)
                    OnlineButton()
                    Button("Apply station settings") {
                        Task {
                            await core.goOffline()
                            core.startEngine()
                        }
                    }
                    NavigationLink("Log") { LogView() }
                } header: { Text("Status") } footer: {
                    Text("Openslot \(Core.version). Received files are in the Files app under Openslot › Received.")
                }
            }
            .navigationTitle("Settings")
        }
    }
}

struct LogView: View {
    @EnvironmentObject var core: Core

    var body: some View {
        ScrollViewReader { proxy in
            List(Array(core.log.enumerated()), id: \.offset) { i, line in
                Text(line).font(.caption.monospaced()).id(i)
            }
            .listStyle(.plain)
            .onAppear { proxy.scrollTo(core.log.count - 1, anchor: .bottom) }
        }
        .navigationTitle("Log")
    }
}

// MARK: the Modem tab: the NR5P XARA app's screens inside Openslot

struct ModemTabView: View {
    @State private var page = 0

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Picker("Page", selection: $page) {
                    Text("Modem").tag(0)
                    Text("Web UI").tag(1)
                    Text("Received").tag(2)
                    Text("Log").tag(3)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal)
                .padding(.vertical, 6)
                switch page {
                case 0: ModemSettingsPage()
                case 1: ModemWebPage()
                case 2: ModemReceivedPage()
                default: ModemLogPage()
                }
            }
            .navigationTitle("Modem")
            .navigationBarTitleDisplayMode(.inline)
        }
    }
}

/// Start/Stop, then every modem setting (saved as it changes; a running modem restarts to use them).
struct ModemSettingsPage: View {
    @EnvironmentObject var core: Core
    @AppStorage(Keys.call) private var call = ""
    @AppStorage(Keys.modemSource) private var source = "builtin"
    @AppStorage(Keys.host) private var host = "192.168.1.10"
    @AppStorage(Keys.port) private var port = 8300
    @AppStorage(Keys.bandwidth) private var bandwidth = "500"
    @AppStorage(Keys.ptt) private var ptt = "vox"
    @AppStorage(Keys.rigctld) private var rigctld = "192.168.1.10:4532"
    @AppStorage(Keys.driveDb) private var driveDb = -5.0
    @AppStorage(Keys.allowBuiltIn) private var allowBuiltIn = false
    @AppStorage(Keys.registered) private var registered = false
    @AppStorage(Keys.monitor) private var monitor = false
    @AppStorage(Keys.monitorFrames) private var monitorFrames = false
    @AppStorage(Keys.monitorCalls) private var monitorCalls = ""
    @AppStorage(Keys.debugLog) private var debugLog = false
    @AppStorage(Keys.recordAudio) private var recordAudio = false
    @AppStorage(Keys.lan) private var lan = false
    @AppStorage(Keys.uiControl) private var uiControl = false
    @State private var changed = false
    @State private var addresses: [String] = []

    private var online: Bool { core.modemConnected || core.modemRunning }
    /// Everything that takes a restart, to notice a change.
    private var restartKey: String {
        "\(source) \(host) \(port) \(ptt) \(rigctld) \(driveDb) \(allowBuiltIn) \(registered) \(monitor) " +
            "\(monitorFrames) \(monitorCalls) \(debugLog) \(recordAudio) \(lan) \(uiControl)"
    }

    var body: some View {
        Form {
            Section {
                HStack {
                    Circle().fill(online ? .green : .secondary).frame(width: 10, height: 10)
                    Text(core.status).font(.callout)
                }
                if source == "builtin" { Text("Audio: \(core.route)").font(.caption).foregroundStyle(.secondary) }
                Button(online ? "Stop" : "Start", role: online ? .destructive : nil) {
                    changed = false
                    Task { if online { await core.goOffline() } else { await core.goOnline() } }
                }
                if online && changed {
                    Button("Restart with the new settings") {
                        changed = false
                        Task { await core.restartWithSettings() }
                    }
                }
            } footer: {
                if online && changed { Text("The modem uses the new settings once it restarts.") }
            }

            Section("Modem") {
                Picker("Modem", selection: $source) {
                    Text("XARA on this phone").tag("builtin")
                    Text("VARA on the network").tag("network")
                }
                Picker("Bandwidth", selection: $bandwidth) {
                    Text("500 Hz").tag("500")
                    Text("2300 Hz").tag("2300")
                    Text("2750 Hz").tag("2750")
                }
                .onChange(of: bandwidth) { core.setBandwidth(bandwidth) }
                if source != "builtin" {
                    TextField("Host", text: $host).autocorrectionDisabled().textInputAutocapitalization(.never).keyboardType(.URL)
                    TextField("Command port", value: $port, format: .number.grouping(.never)).keyboardType(.numberPad)
                }
            }

            if source == "builtin" {
                Section {
                    LabeledContent("Call sign", value: call.uppercased().ifEmpty("set it in Settings"))
                    VStack(alignment: .leading) {
                        Text("TX drive \(driveDb, specifier: "%.1f") dB")
                        Slider(value: $driveDb, in: -30...0, step: 0.5)
                    }
                    Toggle("Registered (I hold a VARA licence)", isOn: $registered)
                } header: { Text("Station") } footer: {
                    Text("Registered: only with a registered copy of VARA. Higher speeds, and links at full speed with other registered stations.")
                }

                Section {
                    Picker("PTT", selection: $ptt) {
                        Text("VOX").tag("vox")
                        Text("rigctld (network CAT)").tag("rigctld")
                    }
                    if ptt == "rigctld" {
                        TextField("rigctld host:port", text: $rigctld).autocorrectionDisabled().textInputAutocapitalization(.never)
                    }
                    Toggle("Allow the iPhone speaker and mic (testing only)", isOn: $allowBuiltIn)
                } header: { Text("Radio") } footer: {
                    Text("XARA transmits into the current audio route. Connect a USB audio interface such as a Digirig; with the toggle off, XARA won't start on the iPhone's own speaker or mic.")
                }

                Section {
                    Toggle("Show other stations' traffic", isOn: $monitor)
                    if monitor {
                        Picker("Show", selection: $monitorFrames) {
                            Text("Text only").tag(false)
                            Text("Every packet").tag(true)
                        }
                        TextField("Calls to name (optional)", text: $monitorCalls)
                            .textInputAutocapitalization(.characters).autocorrectionDisabled()
                    }
                } header: { Text("Receive") } footer: {
                    Text("The Received page shows what the modem decodes on your own links. With this on it also decodes VARA HF traffic between other stations, like VARA's monitor. Answering stations show as #hash unless their call is listed.")
                }

                Section {
                    Toggle("Keep a debug log", isOn: $debugLog)
                    Toggle("Record received audio", isOn: $recordAudio)
                } header: { Text("Troubleshooting") } footer: {
                    Text("The debug log adds a line for every frame, host command and, once a second, the audio levels; recording saves what the modem hears and sends (about 3 MB a minute). Both are in the Files app under Openslot › xara-logs.")
                }

                Section {
                    Toggle("Accept clients on the local network", isOn: $lan)
                    Toggle("Web UI can control the radio", isOn: $uiControl)
                } header: { Text("Clients") } footer: {
                    if lan {
                        Text("Clients elsewhere connect to \((addresses.isEmpty ? ["(no network address)"] : addresses).map { "\($0):8300" }.joined(separator: ", ")) (data 8301, KISS 8100, web UI 8080). The modem takes one client at a time, and Openslot is one. Anyone on this network can use it.")
                    } else {
                        Text("VARA command port 8300, data 8301, KISS 8100, web UI 8080, on this iPhone only.")
                    }
                }
            }
        }
        .onChange(of: restartKey) { changed = true }
        .onAppear { addresses = LocalNetwork.addresses() }
    }
}

struct ModemWebPage: View {
    @EnvironmentObject var core: Core
    @State private var reload = 0

    var body: some View {
        Group {
            if core.modemRunning {
                ModemWebView(url: Settings().webURL, reload: reload)
            } else {
                ContentUnavailableView("Modem stopped", systemImage: "waveform.slash",
                                       description: Text("Start the built-in modem to see its waterfall and status."))
            }
        }
        .toolbar {
            Button { reload += 1 } label: { Image(systemName: "arrow.clockwise") }.disabled(!core.modemRunning)
        }
    }
}

/// XARA's web UI (from XARA's iOS app).
struct ModemWebView: UIViewRepresentable {
    let url: URL
    let reload: Int

    /// Retries until the UI server answers; it comes up a moment after Start.
    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: (URL, Int)?

        func webView(_ w: WKWebView, didFailProvisionalNavigation _: WKNavigation!, withError _: Error) {
            guard let url = loaded?.0 else { return }
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak w] in w?.load(URLRequest(url: url)) }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeUIView(context: Context) -> WKWebView {
        let w = WKWebView()
        w.isOpaque = false
        w.navigationDelegate = context.coordinator
        return w
    }

    func updateUIView(_ w: WKWebView, context: Context) {
        if context.coordinator.loaded.map({ $0 != url || $1 != reload }) ?? true {
            context.coordinator.loaded = (url, reload)
            w.load(URLRequest(url: url))
        }
    }
}

struct ModemReceivedPage: View {
    @EnvironmentObject var core: Core

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                Text(core.received.isEmpty ? "Nothing received yet. Your QSOs also show on the Chat screen." : core.received)
                    .font(.system(.callout, design: .monospaced))
                    .foregroundStyle(core.received.isEmpty ? .secondary : .primary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding()
                Color.clear.frame(height: 1).id("end")
            }
            .onChange(of: core.received) { proxy.scrollTo("end", anchor: .bottom) }
        }
        .toolbar {
            ShareLink(item: core.received)
            Button("Clear") { core.clearReceived() }
        }
    }
}

struct ModemLogPage: View {
    @EnvironmentObject var core: Core

    var body: some View {
        ScrollViewReader { proxy in
            List(core.log.indices, id: \.self) { i in
                Text(core.log[i]).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
            }
            .listStyle(.plain)
            .onChange(of: core.log.count) { if core.log.count > 0 { proxy.scrollTo(core.log.count - 1, anchor: .bottom) } }
        }
        .toolbar {
            ShareLink(item: core.log.joined(separator: "\n"))
            Button("Clear") { core.clearLog() }
        }
    }
}

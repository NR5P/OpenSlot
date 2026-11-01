import SwiftUI

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
                HStack {
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
    @AppStorage(Keys.modemSource) private var source = "builtin"
    @AppStorage(Keys.host) private var host = "192.168.1.10"
    @AppStorage(Keys.port) private var port = 8300
    @AppStorage(Keys.bandwidth) private var bandwidth = "500"
    @AppStorage(Keys.frequency) private var frequency = 14_105_000
    @AppStorage(Keys.ptt) private var ptt = "vox"
    @AppStorage(Keys.rigctld) private var rigctld = "192.168.1.10:4532"
    @AppStorage(Keys.driveDb) private var driveDb = -5.0
    @AppStorage(Keys.allowBuiltIn) private var allowBuiltIn = false
    @AppStorage(Keys.registered) private var registered = false
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
                    Picker("Modem", selection: $source) {
                        Text("XARA on this phone").tag("builtin")
                        Text("VARA on the network").tag("network")
                    }
                    Picker("Bandwidth", selection: $bandwidth) {
                        Text("500 Hz").tag("500")
                        Text("2300 Hz").tag("2300")
                        Text("2750 Hz").tag("2750")
                    }
                    if source == "builtin" {
                        Picker("PTT", selection: $ptt) {
                            Text("VOX").tag("vox")
                            Text("rigctld (network)").tag("rigctld")
                        }
                        if ptt == "rigctld" {
                            TextField("rigctld host:port", text: $rigctld).autocorrectionDisabled().textInputAutocapitalization(.never)
                        }
                        VStack(alignment: .leading) {
                            Text("TX drive \(driveDb, specifier: "%.1f") dB")
                            Slider(value: $driveDb, in: -30...0, step: 0.5)
                        }
                        Toggle("Allow the iPhone speaker and mic (testing only)", isOn: $allowBuiltIn)
                        Toggle("Registered (I hold a VARA licence)", isOn: $registered)
                        Text("Audio: \(core.route)").font(.footnote).foregroundStyle(.secondary)
                    } else {
                        TextField("Host", text: $host).autocorrectionDisabled().textInputAutocapitalization(.never).keyboardType(.URL)
                        TextField("Command port", value: $port, format: .number.grouping(.never)).keyboardType(.numberPad)
                    }
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

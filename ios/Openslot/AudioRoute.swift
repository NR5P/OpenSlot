// From XARA's iOS app (github.com/x0rro/xara, MIT licence): the audio session the modem needs.
import AVFoundation
import Foundation

/// The iOS audio session: xara records and plays through whatever route it has.
enum AudioRoute {
    /// Raw audio at 48 kHz with no voice processing, so the modem hears what the radio sends.
    static func activate() throws {
        let s = AVAudioSession.sharedInstance()
        // Without .defaultToSpeaker, play-and-record on the iPhone's own hardware goes to the
        // earpiece, far too quiet for another device to decode. A USB interface still takes the
        // route when one is connected.
        try s.setCategory(.playAndRecord, mode: .measurement, options: [.defaultToSpeaker])
        try s.setPreferredSampleRate(48_000)
        try s.setPreferredIOBufferDuration(0.01)
        try s.setActive(true)
        // The built-in mic at the bottom, beside the speaker: where another phone's speaker faces.
        // This only picks among the built-in mics; it does not take the route from an interface.
        if let mic = s.availableInputs?.first(where: { $0.portType == .builtInMic }),
           let bottom = mic.dataSources?.first(where: { $0.orientation == .bottom }) {
            try? mic.setPreferredDataSource(bottom)
        }
        if s.isInputGainSettable { try? s.setInputGain(0.5) }
    }

    static func requestMicrophone() async -> Bool {
        await withCheckedContinuation { c in
            AVAudioApplication.requestRecordPermission { c.resume(returning: $0) }
        }
    }

    /// Why the current route is the phone's own hardware, or nil for an external interface.
    static func builtInReason() -> String? {
        let r = AVAudioSession.sharedInstance().currentRoute
        let builtIn: [AVAudioSession.Port: String] = [
            .builtInSpeaker: "the iPhone speaker",
            .builtInReceiver: "the iPhone earpiece",
            .builtInMic: "the iPhone microphone",
        ]
        for p in r.outputs + r.inputs {
            if let why = builtIn[p.portType] { return why }
        }
        return nil
    }

    static func describe() -> String {
        let r = AVAudioSession.sharedInstance().currentRoute
        let name = { (ports: [AVAudioSessionPortDescription]) in
            ports.map(\.portName).joined(separator: ", ").ifEmpty("none")
        }
        return "in: \(name(r.inputs)) · out: \(name(r.outputs))"
    }
}

extension String {
    func ifEmpty(_ s: String) -> String { isEmpty ? s : self }
}

/// This device's IPv4 addresses on Wi-Fi or Ethernet, for clients on the LAN.
enum LocalNetwork {
    static func addresses() -> [String] {
        var out: [String] = []
        var head: UnsafeMutablePointer<ifaddrs>?
        guard getifaddrs(&head) == 0, let first = head else { return out }
        defer { freeifaddrs(head) }
        for p in sequence(first: first, next: { $0.pointee.ifa_next }) {
            let i = p.pointee
            let name = String(cString: i.ifa_name)
            guard let a = i.ifa_addr, a.pointee.sa_family == UInt8(AF_INET),
                  name.hasPrefix("en"), i.ifa_flags & UInt32(IFF_UP) != 0 else { continue }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            if getnameinfo(a, socklen_t(a.pointee.sa_len), &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
                out.append(String(cString: host))
            }
        }
        return out
    }
}

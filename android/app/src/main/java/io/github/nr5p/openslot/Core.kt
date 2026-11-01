package io.github.nr5p.openslot

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.IBinder
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.TimeZone

/** The Rust core (mobile-core): VarAC protocol, session, storage, and the XARA modem. Text is UTF-8 JSON. */
object Native {
    init {
        System.loadLibrary("openslot_mobile")
    }

    external fun version(): String

    /** Starts the engine; null on success, else why not. */
    external fun start(config: ByteArray): ByteArray?
    external fun stop()
    external fun command(json: ByteArray): ByteArray?
    external fun query(json: ByteArray): ByteArray
    external fun nextEvent(): ByteArray?

    /** Starts XARA with its command-line options (a JSON array); null on success, else why not. */
    external fun modemStart(args: ByteArray): ByteArray?
    external fun modemStop(): Int
    external fun modemRunning(): Boolean
    external fun modemNextLog(): ByteArray?
}

private fun String.utf8() = toByteArray(Charsets.UTF_8)
private fun ByteArray.text() = String(this, Charsets.UTF_8)

/** Settings kept across launches. */
class Prefs(context: Context) {
    private val p = context.getSharedPreferences("openslot", Context.MODE_PRIVATE)
    val dataDir: File = context.getExternalFilesDir(null) ?: context.filesDir
    val modemState = File(context.filesDir, "xara")

    var call by string("call", "")
    var name by string("name", "")
    var qth by string("qth", "")
    var locator by string("locator", "")
    /** "builtin" (XARA on this phone) or "network" (a VARA modem elsewhere). */
    var source by string("source", "builtin")
    var host by string("host", "192.168.1.10")
    var port by int("port", 8300)
    var bandwidth by string("bandwidth", "500")
    var frequency by long("frequency", 14_105_000)
    var ptt by string("ptt", "vox")
    var rigctld by string("rigctld", "192.168.1.10:4532")
    var driveDb by int("driveDb", -5)
    var capture by int("capture", 0)
    var playback by int("playback", 0)
    var allowBuiltIn by bool("allowBuiltIn", false)
    var registered by bool("registered", false)
    /** Settings → QSO, as VarAC. */
    var allowInfo by bool("allowInfo", true)
    var allowPings by bool("allowPings", true)
    var verboseSnrAuto by bool("verboseSnrAuto", false)
    /** The slot last chosen in the CQ slot selector (VarAC remembers it). */
    var cqSlot by int("cqSlot", 11)
    var cqSlotWait by int("cqSlotWait", 300)
    var skipSlotSelector by bool("skipSlotSelector", false)
    /** Profile and QSO conduct (Settings), as VarAC. */
    var email by string("email", "")
    var fullCall by string("fullCall", "")
    var welcome by string("welcome", "")
    var welcomeAlways by bool("welcomeAlways", false)
    var callIdMinutes by int("callIdMinutes", 10)
    var idleMinutes by int("idleMinutes", 5)
    var allowLastHeard by bool("allowLastHeard", true)
    var multiBandLastHeard by bool("multiBandLastHeard", false)

    /** Settings → QSO and profile, as the engine's `policy` command and config take them. */
    fun policy(o: JSONObject = JSONObject()): JSONObject = o
        .put("allow_info", allowInfo).put("allow_pings", allowPings).put("verbose_snr_auto_accept", verboseSnrAuto)
        .put("welcome_message", welcome).put("welcome_always", welcomeAlways).put("email", email.trim())
        .put("full_callsign", fullCall.trim().uppercase()).put("call_id_minutes", callIdMinutes)
        .put("idle_disconnect_minutes", idleMinutes).put("allow_last_heard", allowLastHeard)
        .put("multi_band_last_heard", multiBandLastHeard)

    val builtIn get() = source == "builtin"
    val callUpper get() = call.trim().uppercase()
    val callValid: Boolean
        get() = callUpper.matches(Regex("^[A-Z0-9]{3,7}(-[0-9]{1,2})?$")) && callUpper.any { it.isDigit() } &&
            !callUpper.startsWith("N0CALL")

    fun engineConfig(): String = JSONObject()
        .put("data_dir", dataDir.path).put("callsign", callUpper)
        .put("name", name).put("qth", qth).put("locator", locator)
        .put("host", if (builtIn) "127.0.0.1" else host).put("cmd_port", if (builtIn) 8300 else port)
        .put("bandwidth", bandwidth).put("frequency", frequency)
        .let { policy(it) }
        .toString()

    /** XARA's options: audio in and out through the chosen devices (by AAudio id). */
    fun modemArguments(): String {
        val a = mutableListOf("--call", callUpper, "--hf", bandwidth)
        a += listOf("--audio", "aaudio", "--capture", "$capture", "--playback", "$playback", "--radio")
        a += listOf("--ptt", if (ptt == "rigctld") "rigctld:$rigctld" else "vox")
        a += listOf("--drive-db", "$driveDb")
        a += listOf("--addr", "127.0.0.1", "--port", "8300", "--kiss-port", "8100")
        a += listOf("--ui-port", "8080", "--ui-addr", "127.0.0.1")
        if (registered) a += "--registered"
        a += listOf("--state-dir", modemState.path)
        return JSONArray(a).toString()
    }

    private fun string(k: String, d: String) = object : kotlin.properties.ReadWriteProperty<Any?, String> {
        override fun getValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>) = p.getString(k, d) ?: d
        override fun setValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>, value: String) =
            p.edit().putString(k, value).apply()
    }

    private fun int(k: String, d: Int) = object : kotlin.properties.ReadWriteProperty<Any?, Int> {
        override fun getValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>) = p.getInt(k, d)
        override fun setValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>, value: Int) =
            p.edit().putInt(k, value).apply()
    }

    private fun long(k: String, d: Long) = object : kotlin.properties.ReadWriteProperty<Any?, Long> {
        override fun getValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>) = p.getLong(k, d)
        override fun setValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>, value: Long) =
            p.edit().putLong(k, value).apply()
    }

    private fun bool(k: String, d: Boolean) = object : kotlin.properties.ReadWriteProperty<Any?, Boolean> {
        override fun getValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>) = p.getBoolean(k, d)
        override fun setValue(thisRef: Any?, property: kotlin.reflect.KProperty<*>, value: Boolean) =
            p.edit().putBoolean(k, value).apply()
    }
}

/** An audio device as the app offers it; XARA opens it by id. */
data class Device(val id: Int, val label: String, val builtIn: Boolean, val usb: Boolean)

object AudioDevices {
    private val builtInTypes = setOf(
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER, AudioDeviceInfo.TYPE_BUILTIN_EARPIECE,
        AudioDeviceInfo.TYPE_BUILTIN_MIC, AudioDeviceInfo.TYPE_TELEPHONY,
    )
    private val usbTypes = setOf(
        AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET, AudioDeviceInfo.TYPE_USB_ACCESSORY,
    )

    fun list(context: Context, inputs: Boolean): List<Device> {
        val am = context.getSystemService(AudioManager::class.java)
        val flag = if (inputs) AudioManager.GET_DEVICES_INPUTS else AudioManager.GET_DEVICES_OUTPUTS
        return am.getDevices(flag).map {
            val label = when (it.type) {
                AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> "Phone speaker"
                AudioDeviceInfo.TYPE_BUILTIN_EARPIECE -> "Phone earpiece"
                AudioDeviceInfo.TYPE_BUILTIN_MIC -> "Phone mic"
                AudioDeviceInfo.TYPE_TELEPHONY -> "Phone call audio"
                in usbTypes -> "${it.productName} (USB)"
                else -> "${it.productName} (type ${it.type})"
            }
            Device(it.id, label, it.type in builtInTypes, it.type in usbTypes)
        }
    }
}

/**
 * VarAC's slot system: QSOs move to slots around the calling frequency, 750 Hz apart for
 * VARA HF; 1-5 are below it, 11-15 above, 0 is the calling frequency itself.
 */
object Slots {
    val byFrequency = listOf(5, 4, 3, 2, 1, 0, 11, 12, 13, 14, 15)
    const val STEP = 750

    fun offset(slot: Int) = when (slot) {
        in 1..5 -> -slot * STEP
        in 11..15 -> (slot - 10) * STEP
        else -> 0
    }

    fun label(slot: Int) = if (slot == 0) "CF" else String.format(Locale.US, "slot %d (%+d Hz)", slot, offset(slot))
}

/** MHz with the digits a frequency needs: 14.105, 14.1065, 14.10275. */
fun mhz(hz: Long): String {
    var s = String.format(Locale.US, "%.5f", hz / 1e6)
    while (s.endsWith("0") && s.length - s.indexOf('.') > 4) s = s.dropLast(1)
    return s
}

/** Standing by on our slot after a CQ, until a reply comes or [until] (epoch ms). */
data class SlotWait(val slot: Int, val label: String, val hz: Long, val cfHz: Long, val until: Long)
/** A QSY invitation waiting for the operator (ours or the partner's): move to [targetHz]. */
data class QsyInvitation(val targetHz: Long, val inviter: Boolean, val seconds: Int)

data class ChatLine(val from: String, val text: String, val time: String, val outgoing: Boolean, val system: Boolean)
/** A station heard calling CQ or beaconing; a CQ may name the slot its caller waits on (VarAC). */
data class Heard(
    val call: String, val time: String, val bandwidth: String, val beacon: Boolean, val count: Int,
    val slot: Int = 0, val slotLabel: String = "", val slotHz: Long? = null,
) {
    fun withSlot(o: JSONObject) = copy(
        slot = o.optInt("slot"),
        slotLabel = o.optString("slot_label").takeIf { it.isNotEmpty() && it != "null" } ?: "",
        slotHz = if (o.isNull("slot_hz")) null else o.optLong("slot_hz"),
    )
}
data class BroadcastLine(val from: String, val to: String, val message: String, val time: String)
data class VMailItem(
    val guid: String, val from: String, val to: String, val subject: String, val body: String,
    val time: String, val read: Boolean, val urgent: Boolean,
)

/** The engine and the built-in modem, as the UI sees them. */
object Core {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private lateinit var prefs: Prefs
    private var connectDeadline = 0L

    val engineRunning = MutableStateFlow(false)
    val modemRunning = MutableStateFlow(false)
    val modemConnected = MutableStateFlow(false)
    val status = MutableStateFlow("Stopped")
    val partner = MutableStateFlow<String?>(null)
    val partnerInfo = MutableStateFlow("")
    val ptt = MutableStateFlow(false)
    val busy = MutableStateFlow(false)
    val typing = MutableStateFlow(false)
    val chat = MutableStateFlow<List<ChatLine>>(emptyList())
    val heard = MutableStateFlow<List<Heard>>(emptyList())
    val broadcasts = MutableStateFlow<List<BroadcastLine>>(emptyList())
    val log = MutableStateFlow<List<String>>(emptyList())
    val unread = MutableStateFlow(0)
    val vmailRevision = MutableStateFlow(0)
    val fileOffer = MutableStateFlow<String?>(null)
    val qsyRequest = MutableStateFlow<String?>(null)
    /** Verbose SNR is running on this link (an SNR report each minute). */
    val verboseSnr = MutableStateFlow(false)
    /** The partner invited us to verbose SNR: answer with answerVerboseSnr. */
    val verboseSnrInvite = MutableStateFlow<String?>(null)
    /** The CQ slot selector is open. */
    val showCqSelector = MutableStateFlow(false)
    val slotWait = MutableStateFlow<SlotWait?>(null)
    /** We called and the partner allows auto-QSY: pick the slot (the calling frequency in Hz). */
    val qsyInviteFrom = MutableStateFlow<Long?>(null)
    var qsyInviteOnCf = true
        private set
    /** "Your QSO is over. Back to the calling frequency?" (its frequency in Hz). */
    val qsyBack = MutableStateFlow<Long?>(null)
    /** VarAC's "QSY NOW" countdown: answer with answerQsy (rejected by itself when it runs out). */
    val qsyInvitation = MutableStateFlow<QsyInvitation?>(null)
    /** We hold VMail for the partner but the link is on the calling frequency. */
    val vmailRelayAlert = MutableStateFlow<String?>(null)
    private var typingUntil = 0L

    val version: String get() = Native.version()

    fun init(context: Context) {
        if (::prefs.isInitialized) return
        prefs = Prefs(context.applicationContext)
        startEngine()
        scope.launch {
            while (isActive) {
                tick()
                delay(200)
            }
        }
    }

    fun startEngine() {
        if (!prefs.callValid) {
            status.value = "Set your call sign in Settings."
            return
        }
        val e = Native.start(prefs.engineConfig().utf8())
        if (e == null) {
            engineRunning.value = true
            refreshUnread()
            loadHeardHistory()
        } else {
            status.value = "Couldn't start: ${e.text()}"
        }
    }

    fun send(cmd: JSONObject) {
        Native.command(cmd.toString().utf8())?.let { addSystem(it.text()) }
    }

    fun send(name: String, vararg kv: Pair<String, Any?>) {
        val o = JSONObject().put("cmd", name)
        kv.forEach { (k, v) -> o.put(k, v ?: JSONObject.NULL) }
        send(o)
    }

    fun query(q: JSONObject): String = Native.query(q.toString().utf8()).text()

    // ---- going on the air ----

    fun goOnline(context: Context) {
        if (!prefs.callValid) {
            status.value = "Set your call sign in Settings."
            return
        }
        if (!engineRunning.value) startEngine()
        if (prefs.builtIn && !modemRunning.value) {
            val inputs = AudioDevices.list(context, true)
            val outputs = AudioDevices.list(context, false)
            val cap = inputs.find { it.id == prefs.capture } ?: inputs.firstOrNull { it.usb }
            val play = outputs.find { it.id == prefs.playback } ?: outputs.firstOrNull { it.usb }
            if (cap == null || play == null) {
                status.value = "Choose the audio in and out in Settings (plug in the radio's USB audio interface)."
                return
            }
            if (!prefs.allowBuiltIn && (cap.builtIn || play.builtIn)) {
                status.value = "Not transmitting into the phone's own speaker or mic. Choose the radio's USB audio."
                return
            }
            prefs.capture = cap.id
            prefs.playback = play.id
            prefs.modemState.mkdirs()
            ContextCompat.startForegroundService(context, Intent(context, ModemService::class.java).putExtra(ModemService.BUILTIN, true))
            status.value = "XARA modem starting…"
            // ModemService starts the modem and then connects the engine (startModemAndConnect).
            return
        }
        // A modem on the network: the service only keeps the app (and its link) alive in the background.
        ContextCompat.startForegroundService(context, Intent(context, ModemService::class.java).putExtra(ModemService.BUILTIN, false))
        connectEngine()
    }

    /** Called by the service once it is in the foreground (Android requires that before recording). */
    internal fun startModemAndConnect() {
        val e = Native.modemStart(prefs.modemArguments().utf8())
        if (e != null) {
            status.value = "Modem: ${e.text()}"
            ModemService.instance?.stopSelf()
            return
        }
        modemRunning.value = true
        scope.launch {
            delay(800)
            connectEngine()
        }
    }

    private fun connectEngine() {
        status.value = "Connecting to the modem…"
        connectDeadline = System.currentTimeMillis() + 15_000
        send("connect_modem")
    }

    fun goOffline(context: Context) {
        connectDeadline = 0
        send("disconnect_modem")
        scope.launch {
            stopModem()
            context.stopService(Intent(context, ModemService::class.java))
            status.value = "Offline"
        }
    }

    private suspend fun stopModem() {
        if (!modemRunning.value) return
        val code = withContext(Dispatchers.IO) { Native.modemStop() }
        modemRunning.value = false
        if (code != 0) addLog("XARA stopped with exit $code")
    }

    fun applySettings(context: Context) {
        goOffline(context)
        scope.launch {
            delay(500)
            Native.stop()
            engineRunning.value = false
            startEngine()
        }
    }

    // ---- actions ----

    fun connect(call: String) = send("connect", "call" to call)
    fun ping(call: String) = send("ping", "call" to call)
    fun chat(text: String) = send("chat", "text" to text)
    fun setVerboseSnr(on: Boolean) = send("verbose_snr", "on" to on)

    /** Call CQ: VarAC first asks which slot to wait on afterwards, unless that's switched off. */
    fun cq() {
        if (prefs.skipSlotSelector) callCq(0) else showCqSelector.value = true
    }

    fun callCq(slot: Int) {
        prefs.cqSlot = slot
        send("cq", "slot" to slot, "wait_secs" to prefs.cqSlotWait)
    }

    /** Stop waiting on the slot: back to the calling frequency. */
    fun returnToCf() = send("slot_return")

    fun answerVerboseSnr(accept: Boolean) {
        verboseSnrInvite.value = null
        send("verbose_snr_answer", "accept" to accept)
    }

    /** Settings → QSO and profile changed: applies at once (MYCALL is re-registered for pings). */
    fun applyPolicy() {
        if (!engineRunning.value) return
        send(prefs.policy(JSONObject().put("cmd", "policy")))
    }

    /** Our QSY invitation in the opening handshake (0 = stay on the calling frequency). */
    fun qsyInvite(slot: Int) {
        qsyInviteFrom.value = null
        send("qsy_invite", "slot" to slot)
    }

    /** Ask the partner to QSY by [offsetHz] (e.g. to relay VMail off the calling frequency). */
    fun qsyRequest(offsetHz: Int) = send("qsy_request", "offset_hz" to offsetHz)

    /** The operator's answer in the "QSY NOW" countdown. */
    fun answerQsy(accept: Boolean) {
        qsyInvitation.value = null
        send("qsy_answer", "accept" to accept)
    }

    /** After the QSO: the operator retuned to the calling frequency. */
    fun backToCf(hz: Long) {
        qsyBack.value = null
        send("return_cf", "hz" to hz)
    }

    /**
     * VarAC's INQUIRE: last_heard, last_heard_call (with [call]), parked, locator, gps,
     * version, schedule; "info" asks for the partner's station info.
     */
    fun inquire(what: String, call: String? = null) {
        if (what == "info") return send("info_request")
        send("inquire", "what" to what, "call" to call)
    }

    /**
     * Answer a CQ that names a slot (VarAC): the operator has tuned there; the engine logs
     * that frequency and goes back to the calling one after the QSO.
     */
    fun connectOnSlot(h: Heard) {
        val hz = h.slotHz ?: return connect(h.call)
        send("connect", "call" to h.call, "slot_hz" to hz)
    }

    fun vmail(to: String, subject: String, body: String, urgent: Boolean) =
        send("vmail", "to" to to, "subject" to subject, "body" to body, "urgent" to urgent)

    fun vmails(folder: String): List<VMailItem> {
        val a = JSONArray(query(JSONObject().put("q", "vmails").put("folder", folder)).ifBlank { "[]" }.let { if (it == "null") "[]" else it })
        return (0 until a.length()).map {
            val o = a.getJSONObject(it)
            VMailItem(
                o.optString("guid"), o.optString("from"), o.optString("to"), o.optString("subject"),
                o.optString("body"), o.optString("time"), o.optBoolean("read", true), o.optBoolean("urgent"),
            )
        }
    }

    fun markRead(guid: String) {
        query(JSONObject().put("q", "vmail_read").put("guid", guid))
        refreshUnread()
    }

    fun delete(guid: String) {
        query(JSONObject().put("q", "vmail_delete").put("guid", guid))
        refreshUnread()
        vmailRevision.value++
    }

    fun clearChat() {
        chat.value = emptyList()
    }

    private fun refreshUnread() {
        unread.value = query(JSONObject().put("q", "unread")).toIntOrNull() ?: 0
    }

    private fun loadHeardHistory() {
        val raw = query(JSONObject().put("q", "heard").put("limit", 300))
        val a = runCatching { JSONArray(raw) }.getOrNull() ?: return
        val seen = LinkedHashMap<String, Heard>()
        for (i in 0 until a.length()) {
            val o = a.getJSONObject(i)
            val call = o.optString("call")
            if (call.isEmpty() || seen.containsKey(call)) continue
            seen[call] = Heard(call, o.optString("time"), o.optString("bandwidth"), o.optBoolean("beacon"), 1).withSlot(o)
        }
        heard.value = seen.values.toList()
    }

    // ---- events ----

    private fun tick() {
        while (true) {
            val e = Native.nextEvent() ?: break
            runCatching { handle(JSONObject(e.text())) }
        }
        while (true) {
            val l = Native.modemNextLog() ?: break
            addLog(l.text())
        }
        if (modemRunning.value && !Native.modemRunning()) {
            modemRunning.value = false
            status.value = "XARA stopped: " +
                (log.value.lastOrNull { it.startsWith("xara: ") }?.removePrefix("xara: ") ?: "see Log")
            ModemService.instance?.stopSelf()
        }
        if (typing.value && System.currentTimeMillis() > typingUntil) typing.value = false
    }

    private fun handle(e: JSONObject) {
        val time = e.optString("time").ifEmpty { now() }
        when (e.optString("type")) {
            "modem" -> {
                val up = e.optBoolean("connected")
                modemConnected.value = up
                if (up) {
                    connectDeadline = 0
                    status.value = "On the air as ${prefs.callUpper}"
                } else if (e.has("error")) {
                    if (System.currentTimeMillis() < connectDeadline) {
                        // The modem may still be opening its ports: try again shortly
                        scope.launch {
                            delay(1000)
                            if (!modemConnected.value && connectDeadline != 0L) send("connect_modem")
                        }
                    } else {
                        status.value = "Modem: ${e.optString("error")}"
                    }
                } else {
                    status.value = "Offline"
                }
                if (!up) {
                    partner.value = null
                    slotWait.value = null
                }
            }
            "connected" -> {
                qsyBack.value = null
                partner.value = e.optString("call")
                partnerInfo.value = ""
            }
            "disconnected" -> {
                partner.value = null
                partnerInfo.value = ""
                typing.value = false
                qsyInviteFrom.value = null
                vmailRelayAlert.value = null
            }
            "chat" -> {
                chat.value = (chat.value + ChatLine(e.optString("from"), e.optString("text"), time, e.optBoolean("outgoing"), false)).takeLast(2000)
                typing.value = false
            }
            "system" -> addSystem(e.optString("text"), time)
            "typing" -> {
                typing.value = true
                typingUntil = System.currentTimeMillis() + 8000
            }
            "opening" -> {
                val parts = listOfNotNull(
                    e.optString("name").takeIf { it.isNotEmpty() && it != "null" },
                    e.optString("qth").takeIf { it.isNotEmpty() && it != "null" },
                    e.optString("locator").takeIf { it.isNotEmpty() && it != "null" },
                    if (e.has("snr")) String.format(Locale.US, "%+d dB", e.optInt("snr")) else null,
                )
                partnerInfo.value = parts.joinToString(" · ")
                addSystem("de ${e.optString("call")} ${partnerInfo.value}", time)
            }
            "cq" -> {
                val call = e.optString("call")
                val old = heard.value.find { it.call == call }
                val h = Heard(call, time, e.optString("bandwidth"), e.optBoolean("beacon"), (old?.count ?: 0) + 1).withSlot(e)
                heard.value = listOf(h) + heard.value.filter { it.call != call }
            }
            "broadcast" -> broadcasts.value = (listOf(
                BroadcastLine(e.optString("from"), e.optString("to"), e.optString("message"), time),
            ) + broadcasts.value).take(500)
            "vmail" -> addSystem("VMail from ${e.optString("from")}: ${e.optString("subject")}", time)
            "vmail_changed" -> {
                refreshUnread()
                vmailRevision.value++
            }
            "file_offer" -> fileOffer.value = "${e.optString("name")} (${e.optLong("size")} bytes)" +
                if (e.optBoolean("attachment")) ", over your incoming files size limit. Rejecting it rejects the whole VMail" else ""
            "file_offer_closed" -> fileOffer.value = null
            "qsy_request" -> qsyRequest.value = if (e.has("offset_hz")) {
                String.format(Locale.US, "QSY %+d Hz", e.optInt("offset_hz"))
            } else {
                "QSY to ${mhz(e.optLong("freq_hz"))} MHz"
            }
            "slot_wait" -> slotWait.value = SlotWait(
                e.optInt("slot"), e.optString("label"), e.optLong("hz"), e.optLong("cf_hz"),
                System.currentTimeMillis() + e.optLong("seconds") * 1000,
            )
            "slot_wait_end" -> slotWait.value = null
            "qsy_invite_request" -> {
                qsyInviteOnCf = e.optBoolean("on_cf", true)
                qsyInviteFrom.value = e.optLong("cf_hz", prefs.frequency)
            }
            "qsy_back" -> qsyBack.value = e.optLong("cf_hz")
            "qsy_invitation" -> qsyInvitation.value =
                QsyInvitation(e.optLong("target_hz"), e.optBoolean("inviter"), e.optInt("seconds", 30))
            "qsy_invitation_closed" -> qsyInvitation.value = null
            "vmail_relay_alert" -> vmailRelayAlert.value = e.optString("call")
            "verbose_snr" -> verboseSnr.value = e.optBoolean("on")
            "verbose_snr_invite" -> verboseSnrInvite.value = e.optString("call").ifEmpty { partner.value ?: "The station" }
            "ptt" -> ptt.value = e.optBoolean("on")
            "busy" -> busy.value = e.optBoolean("on")
            "log" -> addLog(e.optString("text"))
        }
    }

    private fun addSystem(text: String, time: String = now()) {
        chat.value = (chat.value + ChatLine("", text, time, outgoing = false, system = true)).takeLast(2000)
    }

    private fun addLog(s: String) {
        log.value = (log.value + s).takeLast(3000)
    }

    private fun now(): String = SimpleDateFormat("HH:mm", Locale.US).apply { timeZone = TimeZone.getTimeZone("UTC") }.format(Date())
}

/**
 * A foreground service, so Android keeps the modem and its microphone access alive while the app
 * is in the background.
 */
class ModemService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
    }

    override fun onDestroy() {
        instance = null
        super.onDestroy()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val builtIn = intent?.getBooleanExtra(BUILTIN, true) != false
        // XARA records the radio's audio; a network modem is just a connection to another device.
        val type = if (builtIn) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE
        startForeground(1, notification(builtIn), type)
        if (builtIn && !Core.modemRunning.value) Core.startModemAndConnect()
        return START_NOT_STICKY
    }

    private fun notification(builtIn: Boolean): Notification {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CHANNEL, "Modem", NotificationManager.IMPORTANCE_LOW))
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Openslot is on the air")
            .setContentText(if (builtIn) "The XARA modem is listening and may transmit." else "Connected to the modem on the network.")
            .setSmallIcon(R.drawable.ic_stat_openslot)
            .setContentIntent(open)
            .setOngoing(true)
            .build()
    }

    companion object {
        const val BUILTIN = "builtin"
        private const val CHANNEL = "modem"
        var instance: ModemService? = null
    }
}

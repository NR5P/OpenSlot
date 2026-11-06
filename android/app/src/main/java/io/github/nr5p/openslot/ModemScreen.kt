package io.github.nr5p.openslot

import android.Manifest
import android.annotation.SuppressLint
import android.content.Intent
import android.content.pm.PackageManager
import android.webkit.WebResourceError
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Card
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SecondaryTabRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import java.net.Inet4Address
import java.net.NetworkInterface

// ---- the Modem tab: the NR5P XARA app's screens inside Openslot ----

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ModemScreen() {
    var page by rememberSaveable { mutableIntStateOf(0) }
    Column(Modifier.fillMaxSize()) {
        SecondaryTabRow(page) {
            listOf("Modem", "Web UI", "Received", "Log").forEachIndexed { i, label ->
                Tab(page == i, { page = i }, text = { Text(label) })
            }
        }
        when (page) {
            0 -> ModemSettingsPage()
            1 -> ModemWebPage()
            2 -> ModemReceivedPage()
            else -> ModemLogPage()
        }
    }
}

/** Start/Stop, then every modem setting (saved as it changes; a running modem restarts to use them). */
@Composable
fun ModemSettingsPage() {
    val context = LocalContext.current
    val prefs = remember { Prefs(context) }
    val connected by Core.modemConnected.collectAsState()
    val running by Core.modemRunning.collectAsState()
    val status by Core.status.collectAsState()
    var changed by remember { mutableStateOf(false) }
    var source by remember { mutableStateOf(prefs.source) }
    var host by remember { mutableStateOf(prefs.host) }
    var port by remember { mutableStateOf(prefs.port.toString()) }
    var bandwidth by remember { mutableStateOf(prefs.bandwidth) }
    var modemType by remember { mutableStateOf(prefs.modemType) }
    var fmWidth by remember { mutableStateOf(prefs.fmWidthArg) }
    var digipeatVia by remember { mutableStateOf(prefs.digipeatVia) }
    var ptt by remember { mutableStateOf(prefs.ptt) }
    var rigctld by remember { mutableStateOf(prefs.rigctld) }
    var drive by remember { mutableStateOf(prefs.driveDb.toFloat()) }
    var capture by remember { mutableIntStateOf(prefs.capture) }
    var playback by remember { mutableIntStateOf(prefs.playback) }
    var allowBuiltIn by remember { mutableStateOf(prefs.allowBuiltIn) }
    var registered by remember { mutableStateOf(prefs.registered) }
    var monitor by remember { mutableStateOf(prefs.monitor) }
    var monitorFrames by remember { mutableStateOf(prefs.monitorFrames) }
    var monitorCalls by remember { mutableStateOf(prefs.monitorCalls) }
    var debugLog by remember { mutableStateOf(prefs.debugLog) }
    var recordAudio by remember { mutableStateOf(prefs.recordAudio) }
    var lan by remember { mutableStateOf(prefs.lan) }
    var uiControl by remember { mutableStateOf(prefs.uiControl) }
    val inputs = remember { AudioDevices.list(context, true) }
    val outputs = remember { AudioDevices.list(context, false) }
    val online = connected || running
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { got ->
        if (got[Manifest.permission.RECORD_AUDIO] == true) Core.goOnline(context)
        else Core.status.value = "Microphone access is needed to hear the radio."
    }
    /** Keeps a setting (an online modem picks it up on Restart). */
    fun set(apply: () -> Unit) {
        apply()
        changed = true
    }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(10.dp).background(if (running || connected) Color(0xFF34C759) else Color.Gray, CircleShape))
                    Spacer(Modifier.size(8.dp))
                    Text(status)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(
                        onClick = {
                            // (settings changed while stopped: the engine takes them before going on the air)
                            if (!online && changed) Core.reloadEngine()
                            if (online) {
                                Core.goOffline(context)
                            } else {
                                val need = listOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS)
                                if (!prefs.builtIn || context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                                    Core.goOnline(context)
                                } else {
                                    askMic.launch(need.toTypedArray())
                                }
                            }
                            changed = false
                        },
                        colors = if (online) ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error) else ButtonDefaults.buttonColors(),
                    ) { Text(if (online) "Stop" else "Start") }
                    if (online && changed) {
                        Button({ changed = false; Core.restartWithSettings(context) }) { Text("Restart with the new settings") }
                    }
                }
                if (online && changed) Note("The modem uses the new settings once it restarts.")
            }
        }

        Header("Modem")
        Choice("XARA on this phone", source == "builtin") { set { source = "builtin"; prefs.source = source } }
        Choice("VARA on the network", source == "network") { set { source = "network"; prefs.source = source } }
        Text("VARA", style = MaterialTheme.typography.labelLarge)
        Row {
            listOf("hf" to "HF", "fm" to "FM").forEach { (t, label) ->
                Choice(label, modemType == t) { set { modemType = t; prefs.modemType = t; Core.fm.value = t == "fm" } }
            }
        }
        if (modemType == "fm") {
            if (source == "builtin") {
                Text("FM mode", style = MaterialTheme.typography.labelLarge)
                Row {
                    Choice("Narrow", fmWidth == "narrow") { set { fmWidth = "narrow"; prefs.fmWidth = fmWidth } }
                    Choice("Wide", fmWidth == "wide") { set { fmWidth = "wide"; prefs.fmWidth = fmWidth } }
                }
            }
            OutlinedTextField(digipeatVia, { digipeatVia = it.uppercase(); set { prefs.digipeatVia = digipeatVia } }, Modifier.fillMaxWidth(),
                label = { Text("Digipeat via (optional)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false))
            Note(
                if (source == "builtin") {
                    "VARA FM, for VHF/UHF radios: narrow for most FM radios through the mic and speaker jacks, wide for a " +
                        "9600-baud data port. CQs and beacons go through the digipeaters you list (space between them); to " +
                        "connect through one, call \"CALL VIA DIGI\". No 500/2300 Hz choice and no calling-frequency rules on FM."
                } else {
                    "Run VARA FM on the network computer. CQs and beacons go through the digipeaters you list (space between " +
                        "them); to connect through one, call \"CALL VIA DIGI\"."
                },
            )
        } else {
            Text("Bandwidth", style = MaterialTheme.typography.labelLarge)
            Row {
                listOf("500", "2300", "2750").forEach { bw ->
                    // (applies at once: the modem switches when connected)
                    Choice("$bw Hz", bandwidth == bw) { bandwidth = bw; Core.setBandwidth(bw) }
                }
            }
        }
        if (source != "builtin") {
            OutlinedTextField(host, { host = it; set { prefs.host = it.trim() } }, Modifier.fillMaxWidth(), label = { Text("Host") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri))
            OutlinedTextField(port, { port = it.filter(Char::isDigit); set { prefs.port = port.toIntOrNull() ?: 8300 } }, Modifier.fillMaxWidth(),
                label = { Text("Command port") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
            Note("Start VARA yourself; Openslot connects to it there. The XARA settings aren't used.")
            return@Column
        }

        Header("Station")
        Text("Call sign ${prefs.callUpper.ifEmpty { "(set it in Settings)" }}")
        Text("TX drive ${drive.toInt()} dB", style = MaterialTheme.typography.labelLarge)
        Slider(drive, { drive = it; set { prefs.driveDb = it.toInt() } }, valueRange = -30f..0f, steps = 29)
        Toggle("Registered (I hold a VARA licence)", registered) { set { registered = it; prefs.registered = it } }
        Note("Only with a registered copy of VARA: higher speeds, and links at full speed with other registered stations.")

        Header("Radio")
        DevicePicker("Audio in", inputs, capture) { set { capture = it; prefs.capture = it } }
        DevicePicker("Audio out", outputs, playback) { set { playback = it; prefs.playback = it } }
        Text("PTT", style = MaterialTheme.typography.labelLarge)
        Row {
            Choice("VOX", ptt == "vox") { set { ptt = "vox"; prefs.ptt = ptt } }
            Choice("rigctld (network CAT)", ptt == "rigctld") { set { ptt = "rigctld"; prefs.ptt = ptt } }
        }
        if (ptt == "rigctld") {
            OutlinedTextField(rigctld, { rigctld = it; set { prefs.rigctld = it.trim() } }, Modifier.fillMaxWidth(),
                label = { Text("rigctld host:port") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri))
        }
        Toggle("Allow the phone's own speaker and mic (testing only)", allowBuiltIn) { set { allowBuiltIn = it; prefs.allowBuiltIn = it } }
        Note("XARA transmits into the audio output chosen above. Connect a USB audio interface such as a Digirig; with the " +
            "toggle off, XARA won't start on the phone's own speaker or mic. Key the radio with VOX or rigctld.")

        Header("Receive")
        Toggle("Show other stations' traffic", monitor) { set { monitor = it; prefs.monitor = it } }
        if (monitor) {
            Row {
                Choice("Text only", !monitorFrames) { set { monitorFrames = false; prefs.monitorFrames = false } }
                Choice("Every packet", monitorFrames) { set { monitorFrames = true; prefs.monitorFrames = true } }
            }
            OutlinedTextField(monitorCalls, { monitorCalls = it.uppercase(); set { prefs.monitorCalls = monitorCalls } }, Modifier.fillMaxWidth(),
                label = { Text("Calls to name (optional)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false))
        }
        Note("The Received page shows what the modem decodes on your own links. With this on it also decodes VARA HF " +
            "traffic between other stations, like VARA's monitor. Answering stations show as #hash unless their call is listed.")

        Header("Troubleshooting")
        Toggle("Keep a debug log", debugLog) { set { debugLog = it; prefs.debugLog = it } }
        Toggle("Record received audio", recordAudio) { set { recordAudio = it; prefs.recordAudio = it } }
        Note("The debug log adds a line for every frame, host command and, once a second, the audio levels; recording " +
            "saves what the modem hears and sends (about 3 MB a minute). Both go to xara-logs in the app's storage, read " +
            "with adb shell run-as io.github.nr5p.openslot.")

        Header("Clients")
        Toggle("Accept clients on the local network", lan) { set { lan = it; prefs.lan = it } }
        Toggle("Web UI can control the radio", uiControl) { set { uiControl = it; prefs.uiControl = it } }
        Note(
            if (lan) {
                val hosts = lanAddresses().ifEmpty { listOf("(no network address)") }
                "Clients elsewhere connect to ${hosts.joinToString { "$it:8300" }} (data 8301, KISS 8100, web UI 8080). " +
                    "The modem takes one client at a time, and Openslot is one. Anyone on this network can use it."
            } else {
                "VARA command port 8300, data 8301, KISS 8100, web UI 8080, on this phone only."
            },
        )
    }
}

@SuppressLint("SetJavaScriptEnabled")
@Composable
fun ModemWebPage() {
    val context = LocalContext.current
    val running by Core.modemRunning.collectAsState()
    var reload by remember { mutableIntStateOf(0) }
    if (!running) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            Text("Start the built-in modem to see its waterfall and status.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        return
    }
    val url = remember { Prefs(context).webUrl }
    Box(Modifier.fillMaxSize()) {
        AndroidView(
            factory = {
                WebView(it).apply {
                    settings.javaScriptEnabled = true
                    // The UI server comes up a moment after Start; retry until it answers.
                    webViewClient = object : WebViewClient() {
                        override fun onReceivedError(view: WebView, request: WebResourceRequest, error: WebResourceError) {
                            if (request.isForMainFrame) view.postDelayed({ view.loadUrl(url) }, 1000)
                        }
                    }
                    loadUrl(url)
                }
            },
            update = { if (reload > 0) it.loadUrl(url) },
            modifier = Modifier.fillMaxSize(),
        )
        IconButton({ reload++ }, Modifier.align(Alignment.TopEnd)) { Icon(Icons.Default.Refresh, "Reload") }
    }
}

@Composable
fun ModemReceivedPage() {
    val context = LocalContext.current
    val received by Core.received.collectAsState()
    val scroll = rememberScrollState()
    LaunchedEffect(received.length) { scroll.animateScrollTo(scroll.maxValue) }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), horizontalArrangement = Arrangement.End) {
            TextButton({
                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, received)
                context.startActivity(Intent.createChooser(send, "Share received text"))
            }, enabled = received.isNotEmpty()) { Text("Share") }
            TextButton({ Core.clearReceived() }) { Text("Clear") }
        }
        Surface(Modifier.fillMaxSize()) {
            Text(
                received.ifEmpty { "Nothing received yet. Your QSOs also show on the Chat screen." },
                Modifier.verticalScroll(scroll).padding(12.dp),
                fontFamily = FontFamily.Monospace, fontSize = 13.sp,
                color = if (received.isEmpty()) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.onSurface,
            )
        }
    }
}

@Composable
fun ModemLogPage() {
    val context = LocalContext.current
    val log by Core.log.collectAsState()
    val state = rememberLazyListState()
    LaunchedEffect(log.size) { if (log.isNotEmpty()) state.scrollToItem(log.size - 1) }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), horizontalArrangement = Arrangement.End) {
            TextButton({
                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, log.joinToString("\n"))
                context.startActivity(Intent.createChooser(send, "Share log"))
            }) { Text("Share") }
            TextButton({ Core.clearLog() }) { Text("Clear") }
        }
        LazyColumn(state = state, contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp)) {
            items(log) { Text(it, fontFamily = FontFamily.Monospace, fontSize = 12.sp) }
        }
    }
}

@Composable
private fun Toggle(label: String, on: Boolean, set: (Boolean) -> Unit) =
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Switch(on, set)
    }

@Composable
private fun Note(text: String) =
    Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

/** This phone's addresses on the local network (Wi-Fi), for clients elsewhere. */
private fun lanAddresses(): List<String> = runCatching {
    NetworkInterface.getNetworkInterfaces().toList().filter { it.isUp && !it.isLoopback }
        .flatMap { it.inetAddresses.toList() }.filterIsInstance<Inet4Address>().map { it.hostAddress ?: "" }.filter { it.isNotEmpty() }
}.getOrDefault(emptyList())

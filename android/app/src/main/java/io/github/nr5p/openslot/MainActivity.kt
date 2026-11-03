package io.github.nr5p.openslot

import android.Manifest
import android.content.pm.PackageManager
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.Close
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Campaign
import androidx.compose.material.icons.filled.Chat
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Email
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Router
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.SettingsInputAntenna
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SecondaryTabRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import java.util.Locale
import androidx.compose.runtime.mutableLongStateOf
import kotlinx.coroutines.delay

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        Core.init(this)
        setContent {
            val dark = isSystemInDarkTheme()
            val accent = Color(0xFF339EF2)
            MaterialTheme(colorScheme = if (dark) darkColorScheme(primary = accent) else lightColorScheme(primary = accent)) {
                App()
            }
        }
    }
}

@Composable
fun App() {
    var tab by remember { mutableIntStateOf(0) }
    val unread by Core.unread.collectAsState()
    Scaffold(
        bottomBar = {
            NavigationBar {
                NavigationBarItem(tab == 0, { tab = 0 }, { Icon(Icons.Default.Chat, null) }, label = { Text("Chat") })
                NavigationBarItem(tab == 1, { tab = 1 }, { Icon(Icons.Default.SettingsInputAntenna, null) }, label = { Text("Stations") })
                NavigationBarItem(tab == 2, { tab = 2 }, {
                    BadgedBox(badge = { if (unread > 0) Badge { Text("$unread") } }) { Icon(Icons.Default.Email, null) }
                }, label = { Text("VMail") })
                NavigationBarItem(tab == 3, { tab = 3 }, { Icon(Icons.Default.Router, null) }, label = { Text("Modem") })
                NavigationBarItem(tab == 4, { tab = 4 }, { Icon(Icons.Default.Settings, null) }, label = { Text("Settings") })
            }
        },
    ) { pad ->
        // The status bar draws behind the system bar itself (statusBarsPadding inside it)
        Column(Modifier.padding(bottom = pad.calculateBottomPadding()).fillMaxSize()) {
            StatusBar()
            when (tab) {
                0 -> ChatScreen()
                1 -> StationsScreen()
                2 -> VMailScreen()
                3 -> ModemScreen()
                else -> SettingsScreen()
            }
        }
    }
    Dialogs()
}

@Composable
fun StatusBar() {
    val context = LocalContext.current
    val connected by Core.modemConnected.collectAsState()
    val running by Core.modemRunning.collectAsState()
    val status by Core.status.collectAsState()
    val partner by Core.partner.collectAsState()
    val ptt by Core.ptt.collectAsState()
    val busy by Core.busy.collectAsState()
    val wait by Core.slotWait.collectAsState()
    val askMic = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { got ->
        if (got[Manifest.permission.RECORD_AUDIO] == true) Core.goOnline(context)
        else Core.status.value = "Microphone access is needed to hear the radio."
    }
    Surface(tonalElevation = 2.dp) { Column {
        Row(Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(10.dp).background(if (connected) Color(0xFF34C759) else Color.Gray, CircleShape))
            Spacer(Modifier.size(8.dp))
            Text(partner?.let { "In QSO with $it" } ?: status, style = MaterialTheme.typography.bodySmall, maxLines = 2, modifier = Modifier.weight(1f))
            if (ptt) Pill("TX", Color(0xFFE5484D)) else if (busy) Pill("BUSY", Color(0xFFF2994A))
            if (connected || running) {
                TextButton({ Core.goOffline(context) }) { Text("Go offline") }
            } else {
                TextButton({
                    val prefs = Prefs(context)
                    val need = listOf(Manifest.permission.RECORD_AUDIO, Manifest.permission.POST_NOTIFICATIONS)
                    if (!prefs.builtIn || context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
                        Core.goOnline(context)
                    } else {
                        askMic.launch(need.toTypedArray())
                    }
                }) { Text("Go on air", fontWeight = FontWeight.Bold) }
            }
        }
        wait?.let { SlotWaitBanner(it) }
    } }
}

/** Standing by on our slot after a CQ (VarAC), with the countdown back to the calling frequency. */
@Composable
fun SlotWaitBanner(w: SlotWait) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(w) {
        while (true) {
            now = System.currentTimeMillis()
            delay(1000)
        }
    }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp)
            .background(Color(0x33E8742A), RoundedCornerShape(8.dp)).padding(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text("Waiting on ${w.label} · ${mhz(w.hz)} MHz", fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.bodySmall)
            Text("Back to the calling frequency in ${maxOf(0L, (w.until - now) / 1000)} s", style = MaterialTheme.typography.bodySmall)
        }
        OutlinedButton({ Core.returnToCf() }) { Text("Return now") }
    }
}

@Composable
fun Pill(text: String, color: Color) {
    Text(
        text, color = Color.White, fontSize = 11.sp, fontWeight = FontWeight.Bold,
        modifier = Modifier.background(color, RoundedCornerShape(50)).padding(horizontal = 6.dp, vertical = 1.dp),
    )
}

@Composable
fun Dialogs() {
    val offer by Core.fileOffer.collectAsState()
    val qsy by Core.qsyRequest.collectAsState()
    val vsnrInvite by Core.verboseSnrInvite.collectAsState()
    val cqSelector by Core.showCqSelector.collectAsState()
    val qsyInviteFrom by Core.qsyInviteFrom.collectAsState()
    val qsyBack by Core.qsyBack.collectAsState()
    val qsyInvitation by Core.qsyInvitation.collectAsState()
    val relayAlert by Core.vmailRelayAlert.collectAsState()
    val bwRefused by Core.bwRefused.collectAsState()
    val partner by Core.partner.collectAsState()
    if (bwRefused) {
        // VarAC's box, with the desktop's one-tap switch to 500 Hz
        AlertDialog(
            onDismissRequest = { Core.bwRefused.value = false },
            title = { Text("Bandwidth not allowed!") },
            text = {
                Text("You can't use a 2300/2750Hz Bandwidth on a calling QRG as it may cause QRM to nearby VarAC stations.\n" +
                    "Please note that 500Hz provides great & fast chat experience.\n\n" +
                    "PLEASE SWITCH YOUR BANDWIDTH TO 500HZ OR CHANGE FREQUENCY.")
            },
            confirmButton = { TextButton({ Core.setBandwidth("500"); Core.bwRefused.value = false }) { Text("Switch to 500 Hz") } },
            dismissButton = { TextButton({ Core.bwRefused.value = false }) { Text("OK") } },
        )
    }
    offer?.let {
        AlertDialog(
            onDismissRequest = {},
            title = { Text("File offered") },
            text = { Text("${partner ?: "The station"} offers $it. It is saved in the app's Received folder.") },
            confirmButton = { TextButton({ Core.send("file_accept"); Core.fileOffer.value = null }) { Text("Receive") } },
            dismissButton = { TextButton({ Core.send("file_decline"); Core.fileOffer.value = null }) { Text("Decline") } },
        )
    }
    qsy?.let {
        AlertDialog(
            onDismissRequest = {},
            title = { Text("Frequency change") },
            text = { Text("${partner ?: "The station"} asks to $it. Retune your radio if you accept.") },
            confirmButton = { TextButton({ Core.send("qsy_accept"); Core.qsyRequest.value = null }) { Text("Accept") } },
            dismissButton = { TextButton({ Core.send("qsy_reject"); Core.qsyRequest.value = null }) { Text("Reject") } },
        )
    }
    vsnrInvite?.let {
        AlertDialog(
            onDismissRequest = {},
            title = { Text("Verbose SNR") },
            text = { Text("$it invites you to exchange SNR reports every minute.") },
            confirmButton = { TextButton({ Core.answerVerboseSnr(true) }) { Text("Accept") } },
            dismissButton = { TextButton({ Core.answerVerboseSnr(false) }) { Text("Reject") } },
        )
    }
    if (cqSelector) CqSlotSelector()
    qsyInviteFrom?.let { QsyInvitePicker(it) }
    relayAlert?.let { call ->
        AlertDialog(
            onDismissRequest = { Core.vmailRelayAlert.value = null },
            title = { Text("VMail relay") },
            text = {
                Text("You have VMail waiting for $call. It's relayed only off the calling frequency: " +
                    "invite them to QSY and it goes out automatically after the QSY.")
            },
            confirmButton = { TextButton({ Core.qsyRequest(750); Core.vmailRelayAlert.value = null }) { Text("Ask to QSY +750 Hz") } },
            dismissButton = { TextButton({ Core.vmailRelayAlert.value = null }) { Text("Later") } },
        )
    }
    qsyInvitation?.let { q ->
        AlertDialog(
            onDismissRequest = {},
            title = { Text("QSY NOW") },
            text = {
                Text("A QSY request to ${mhz(q.targetHz)} MHz has been initiated. Tap QSY and retune your radio there now. " +
                    "If you fail to QSY quickly, the link might be closed. Rejected automatically in ${q.seconds} seconds.")
            },
            confirmButton = { TextButton({ Core.answerQsy(true) }) { Text("QSY") } },
            dismissButton = { TextButton({ Core.answerQsy(false) }) { Text("Reject") } },
        )
    }
    qsyBack?.let { hz ->
        AlertDialog(
            onDismissRequest = { Core.qsyBack.value = null },
            title = { Text("QSY back") },
            text = { Text("Your QSO is over. Switch back to the calling frequency, ${mhz(hz)} MHz?") },
            confirmButton = { TextButton({ Core.backToCf(hz) }) { Text("Yes, retuned") } },
            dismissButton = { TextButton({ Core.qsyBack.value = null }) { Text("No") } },
        )
    }
}

/** The VarAC slots around the calling frequency [cf], each with its frequency. */
@Composable
fun SlotList(slot: Int, cf: Long, pick: (Int) -> Unit) {
    Slots.byFrequency.forEach { s ->
        Row(Modifier.fillMaxWidth().clickable { pick(s) }, verticalAlignment = Alignment.CenterVertically) {
            RadioButton(slot == s, { pick(s) })
            Text(if (s == 0) "CF" else "Slot $s", Modifier.weight(1f))
            Text("${mhz(cf + Slots.offset(s))} MHz", style = MaterialTheme.typography.bodySmall)
        }
    }
}

/**
 * VarAC's QSY invitation in the opening: we called, the partner allows auto-QSY, so we
 * choose the slot both stations move to (the phone doesn't tune: retune by hand).
 */
@Composable
fun QsyInvitePicker(cf: Long) {
    val context = LocalContext.current
    var slot by remember { mutableIntStateOf(Prefs(context).cqSlot) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(cf) {
        while (true) {
            now = System.currentTimeMillis()
            delay(1000)
        }
    }
    AlertDialog(
        onDismissRequest = {},
        title = { Text("QSY invitation") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                Text("Staying automatically in ${maxOf(0L, (Core.qsyInviteUntil - now) / 1000)} s",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                SlotList(slot, cf) { slot = it }
                Spacer(Modifier.size(8.dp))
                Text(
                    if (slot == 0 && Core.qsyInviteOnCf) "Slot 0 stays on the calling frequency; links left there are closed after 4 minutes."
                    else if (slot == 0) "Slot 0 stays where you are."
                    else "Both stations move to ${mhz(cf + Slots.offset(slot))} MHz. Retune your radio when the QSY is agreed.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        },
        confirmButton = { TextButton({ Core.qsyInvite(slot) }) { Text("Invite") } },
        dismissButton = { TextButton({ Core.qsyInvite(0) }) { Text("Stay") } },
    )
}

/** VarAC's CQ slot selector: pick the slot you'll wait on after the CQ, then call. */
@Composable
fun CqSlotSelector() {
    val context = LocalContext.current
    val prefs = remember { Prefs(context) }
    val cf = prefs.frequency
    var slot by remember { mutableIntStateOf(prefs.cqSlot) }
    AlertDialog(
        onDismissRequest = { Core.showCqSelector.value = false },
        title = { Text("Call CQ: wait on") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                SlotList(slot, cf) { slot = it }
                Spacer(Modifier.size(8.dp))
                if (slot == 0) {
                    Text("Slot 0 is the calling frequency itself. QSOs on VarAC calling frequencies are prohibited.",
                        color = Color(0xFFE8742A), style = MaterialTheme.typography.bodySmall)
                } else {
                    Text("Your CQ names this slot; you wait there for replies. The phone doesn't tune the radio: " +
                        "after the CQ, tune it to ${mhz(cf + Slots.offset(slot))} MHz. Make sure the slot is in the band and free.",
                        style = MaterialTheme.typography.bodySmall)
                }
            }
        },
        confirmButton = { TextButton({ Core.showCqSelector.value = false; Core.callCq(slot) }) { Text("Call CQ") } },
        dismissButton = { TextButton({ Core.showCqSelector.value = false }) { Text("Cancel") } },
    )
}

// ---- frequency and bandwidth ----

/**
 * The dial frequency (tap it for the picker) and VARA HF's bandwidth buttons, as on the
 * desktop's main screen. The phone doesn't tune the radio: picking a frequency logs it and
 * applies the calling-frequency rules there.
 */
@Composable
fun FrequencyBar() {
    val hz by Core.frequency.collectAsState()
    val bw by Core.bandwidth.collectAsState()
    var picker by remember { mutableStateOf(false) }
    val band = remember(hz) { Core.bandOf(hz) }
    val onCf = remember(hz) { Core.isCallingFrequency(hz) }
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
        Row(Modifier.clickable { picker = true }.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(dial(hz), fontFamily = FontFamily.Monospace, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Icon(Icons.Default.ArrowDropDown, "Change frequency")
        }
        band?.let { Pill(it, MaterialTheme.colorScheme.primary) }
        if (onCf) {
            Spacer(Modifier.size(4.dp))
            Pill("CF", Color(0xFFE8742A))
        }
        Spacer(Modifier.weight(1f))
        Row(Modifier.clip(RoundedCornerShape(50)).border(1.dp, MaterialTheme.colorScheme.outline, RoundedCornerShape(50))) {
            listOf("500", "2300", "2750").forEach { b ->
                val on = bw == b
                Text(
                    b, fontSize = 13.sp, fontWeight = if (on) FontWeight.Bold else FontWeight.Normal, maxLines = 1, softWrap = false,
                    color = if (on) Color.White else MaterialTheme.colorScheme.onSurface,
                    modifier = Modifier.background(if (on) MaterialTheme.colorScheme.primary else Color.Transparent)
                        .clickable { Core.setBandwidth(b) }.padding(horizontal = 8.dp, vertical = 5.dp),
                )
            }
        }
    }
    if (picker) FrequencyPicker(hz) { picker = false }
}

/** The desktop's frequency picker: type any frequency, a VarAC calling frequency, or one of your own. */
@Composable
fun FrequencyPicker(current: Long, close: () -> Unit) {
    var entry by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var mine by remember { mutableStateOf(Core.myFrequencies) }
    var name by remember { mutableStateOf("") }
    fun pick(hz: Long) {
        if (hz != current) Core.setFrequency(hz)
        close()
    }
    fun tune() {
        Core.parseFrequency(entry)?.let { pick(it) } ?: run { error = "Not a frequency: ${entry.trim()}" }
    }
    AlertDialog(
        onDismissRequest = close,
        title = { Text("Frequency") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(
                        entry, { entry = it; error = null }, Modifier.weight(1f), singleLine = true, isError = error != null,
                        placeholder = { Text("14.1055 or 14105.5") },
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal, imeAction = ImeAction.Go),
                        keyboardActions = KeyboardActions(onGo = { tune() }),
                    )
                    Spacer(Modifier.size(8.dp))
                    Button(::tune, enabled = entry.isNotBlank()) { Text("Tune") }
                }
                Text(error ?: "MHz below 1000, kHz below 1,000,000, else Hz", style = MaterialTheme.typography.bodySmall,
                    color = if (error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
                PickerLabel("VarAC calling frequencies")
                Core.callingFrequencies.chunked(2).forEach { row ->
                    Row {
                        row.forEach { cf -> FreqChoice("${cf.band} ${mhz(cf.hz)}", cf.hz == current, Modifier.weight(1f)) { pick(cf.hz) } }
                        if (row.size == 1) Spacer(Modifier.weight(1f))
                    }
                }
                PickerLabel("My frequencies")
                if (mine.isEmpty()) Text("None yet: add the frequency you're on below.", style = MaterialTheme.typography.bodySmall)
                mine.forEach { f ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        val band = Core.bandOf(f.hz) ?: ""
                        FreqChoice(listOf(mhz(f.hz), band, f.name).filter { it.isNotEmpty() }.joinToString("  "), f.hz == current, Modifier.weight(1f)) {
                            pick(f.hz)
                        }
                        IconButton({ mine = mine - f; Core.myFrequencies = mine }) { Icon(Icons.Default.Close, "Remove from my frequencies") }
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(name, { name = it }, Modifier.weight(1f), singleLine = true, placeholder = { Text("Name (optional)") })
                    TextButton({
                        mine = (mine + SavedFrequency(current, name.trim())).sortedBy { it.hz }
                        Core.myFrequencies = mine
                        name = ""
                    }, enabled = mine.none { it.hz == current }) { Text("Add ${mhz(current)}") }
                }
            }
        },
        confirmButton = { TextButton(close) { Text("Close") } },
    )
}

@Composable
private fun PickerLabel(text: String) {
    Text(text, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(top = 8.dp))
}

@Composable
private fun FreqChoice(label: String, selected: Boolean, modifier: Modifier = Modifier, pick: () -> Unit) {
    Text(
        label, fontFamily = FontFamily.Monospace, fontSize = 13.sp, maxLines = 2,
        fontWeight = if (selected) FontWeight.Bold else FontWeight.Normal,
        modifier = modifier.clip(RoundedCornerShape(6.dp))
            .background(if (selected) MaterialTheme.colorScheme.primaryContainer else Color.Transparent)
            .clickable(onClick = pick).padding(horizontal = 6.dp, vertical = 8.dp),
    )
}

// ---- chat ----

@Composable
fun ChatScreen() {
    val chat by Core.chat.collectAsState()
    val partner by Core.partner.collectAsState()
    val info by Core.partnerInfo.collectAsState()
    val connected by Core.modemConnected.collectAsState()
    val typing by Core.typing.collectAsState()
    val verboseSnr by Core.verboseSnr.collectAsState()
    var callTo by remember { mutableStateOf("") }
    var askLastHeard by remember { mutableStateOf(false) }
    var lastHeardCall by remember { mutableStateOf("") }
    if (askLastHeard) {
        AlertDialog(
            onDismissRequest = { askLastHeard = false },
            title = { Text("Last heard") },
            text = {
                OutlinedTextField(lastHeardCall, { lastHeardCall = it.uppercase() }, singleLine = true, label = { Text("Call sign") },
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false))
            },
            confirmButton = {
                TextButton({ Core.inquire("last_heard_call", lastHeardCall); lastHeardCall = ""; askLastHeard = false }) { Text("Ask") }
            },
            dismissButton = { TextButton({ askLastHeard = false }) { Text("Cancel") } },
        )
    }
    var text by remember { mutableStateOf("") }
    var menu by remember { mutableStateOf(false) }
    val list = rememberLazyListState()
    LaunchedEffect(chat.size) { if (chat.isNotEmpty()) list.animateScrollToItem(chat.size - 1) }

    Column(Modifier.fillMaxSize().imePadding()) {
        FrequencyBar()
        val p = partner
        if (p != null) {
            Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(p, style = MaterialTheme.typography.titleMedium)
                    if (info.isNotEmpty()) Text(info, style = MaterialTheme.typography.bodySmall)
                }
                Box {
                    IconButton({ menu = true }) { Icon(Icons.Default.MoreVert, "More") }
                    DropdownMenu(menu, { menu = false }) {
                        DropdownMenuItem(
                            { Text(if (verboseSnr) "Stop verbose SNR" else "Verbose SNR") },
                            { menu = false; Core.setVerboseSnr(!verboseSnr) },
                        )
                        DropdownMenuItem({ Text("Request SNR") }, { menu = false; Core.send("snr_request") })
                        HorizontalDivider()
                        Text("  Inquire", style = MaterialTheme.typography.labelMedium)
                        listOf(
                            "info" to "Station info", "last_heard" to "Last heard (all)", "parked" to "Parked VMail status",
                            "locator" to "Locator", "gps" to "GPS position", "version" to "Version", "schedule" to "Frequency schedule",
                        ).forEach { (what, label) ->
                            DropdownMenuItem({ Text(label) }, { menu = false; Core.inquire(what) })
                        }
                        DropdownMenuItem({ Text("Last heard a station…") }, { menu = false; askLastHeard = true })
                        HorizontalDivider()
                        Text("  Ask to QSY", style = MaterialTheme.typography.labelMedium)
                        listOf(750, 1500, -750, -1500).forEach { hz ->
                            DropdownMenuItem({ Text(String.format(Locale.US, "%+d Hz", hz)) }, { menu = false; Core.qsyRequest(hz) })
                        }
                    }
                }
                OutlinedButton({ Core.send("disconnect") }) { Text("Disconnect") }
            }
        } else {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(
                    callTo, { callTo = it.uppercase() }, Modifier.weight(1f), singleLine = true,
                    placeholder = { Text("Call sign") },
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false),
                )
                Spacer(Modifier.size(8.dp))
                Button({ Core.connect(callTo) }, enabled = connected && callTo.isNotBlank()) { Text("Connect") }
                Box {
                    IconButton({ menu = true }, enabled = connected) { Icon(Icons.Default.MoreVert, "More") }
                    DropdownMenu(menu, { menu = false }) {
                        DropdownMenuItem({ Text("Ping") }, { menu = false; Core.ping(callTo) }, enabled = callTo.isNotBlank())
                        DropdownMenuItem({ Text("Call CQ") }, { menu = false; Core.cq() })
                        DropdownMenuItem({ Text("Send beacon") }, { menu = false; Core.send("beacon") })
                        DropdownMenuItem({ Text("Abort") }, { menu = false; Core.send("abort") })
                        DropdownMenuItem({ Text("Clear chat") }, { menu = false; Core.clearChat() })
                    }
                }
            }
        }
        HorizontalDivider()
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list, contentPadding = androidx.compose.foundation.layout.PaddingValues(8.dp)) {
            items(chat) { ChatRow(it) }
        }
        if (typing && p != null) {
            Text("$p is typing…", style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(horizontal = 12.dp))
        }
        Row(Modifier.fillMaxWidth().padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                text,
                {
                    if (text.isEmpty() && it.isNotEmpty() && p != null) Core.send("typing")
                    text = it
                },
                Modifier.weight(1f), maxLines = 4, enabled = p != null,
                placeholder = { Text(if (p == null) "Not in a QSO" else "Message") },
            )
            Spacer(Modifier.size(8.dp))
            FilledIconButton({
                val t = text.trim()
                if (t.isNotEmpty()) {
                    Core.chat(t)
                    text = ""
                }
            }, enabled = p != null && text.isNotBlank()) { Icon(Icons.AutoMirrored.Filled.Send, "Send") }
        }
    }
}

@Composable
fun ChatRow(line: ChatLine) {
    if (line.system) {
        Text(
            "${line.time}  ${line.text}", style = MaterialTheme.typography.bodySmall, textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp),
        )
        return
    }
    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), horizontalArrangement = if (line.outgoing) Arrangement.End else Arrangement.Start) {
        Column(horizontalAlignment = if (line.outgoing) Alignment.End else Alignment.Start, modifier = Modifier.widthIn(max = 300.dp)) {
            Text("${line.from}  ${line.time}", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(
                line.text,
                color = if (line.outgoing) Color.White else MaterialTheme.colorScheme.onSurface,
                modifier = Modifier
                    .background(
                        if (line.outgoing) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant,
                        RoundedCornerShape(12.dp),
                    )
                    .padding(horizontal = 10.dp, vertical = 6.dp),
            )
        }
    }
}

// ---- stations ----

@Composable
fun StationsScreen() {
    val heard by Core.heard.collectAsState()
    val broadcasts by Core.broadcasts.collectAsState()
    val connected by Core.modemConnected.collectAsState()
    var picked by remember { mutableStateOf<Heard?>(null) }
    var bcast by remember { mutableStateOf(false) }
    var bcastText by remember { mutableStateOf("") }

    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton({ Core.cq() }, enabled = connected) { Text("Call CQ") }
            OutlinedButton({ Core.send("beacon") }, enabled = connected) { Text("Beacon") }
            OutlinedButton({ bcast = true }, enabled = connected) { Icon(Icons.Default.Campaign, null); Text(" Broadcast") }
        }
        LazyColumn(Modifier.fillMaxSize()) {
            item { Header("Heard (CQ and beacons)") }
            if (heard.isEmpty()) item { Hint("Listening… stations calling CQ or beaconing show up here.") }
            items(heard) { h ->
                Row(Modifier.fillMaxWidth().clickable { picked = h }.padding(horizontal = 16.dp, vertical = 10.dp)) {
                    Column(Modifier.weight(1f)) {
                        Text(h.call, fontWeight = FontWeight.SemiBold)
                        Text("${if (h.beacon) "Beacon" else "CQ"} · ${h.bandwidth} Hz", style = MaterialTheme.typography.bodySmall)
                        if (h.slot != 0 && !h.beacon) {
                            Text("Waits on ${h.slotLabel}", style = MaterialTheme.typography.bodySmall, color = Color(0xFFE8742A))
                        }
                    }
                    Text(h.time, style = MaterialTheme.typography.bodySmall)
                }
                HorizontalDivider()
            }
            item { Header("Broadcasts") }
            if (broadcasts.isEmpty()) item { Hint("No broadcasts heard yet") }
            items(broadcasts) { b ->
                Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                    Text("${b.from} → ${b.to}  ${b.time}", style = MaterialTheme.typography.bodySmall)
                    Text(b.message)
                }
            }
        }
    }
    picked?.let { h ->
        // VarAC: a CQ naming a slot is answered there, so the operator tunes first
        val slotHz = h.slotHz.takeIf { h.slot != 0 && !h.beacon }
        AlertDialog(
            onDismissRequest = { picked = null },
            title = { Text(h.call) },
            text = slotHz?.let { hz ->
                {
                    Text("Called CQ at ${h.time} and waits on ${h.slotLabel}. Tune your radio to ${mhz(hz)} MHz (USB), " +
                        "then connect. Retune to the calling frequency after the QSO.")
                }
            },
            confirmButton = {
                TextButton({ Core.connectOnSlot(h); picked = null }, enabled = connected) {
                    Text(if (slotHz != null) "Tuned, connect" else "Connect")
                }
            },
            dismissButton = { TextButton({ Core.ping(h.call); picked = null }, enabled = connected) { Text("Ping") } },
        )
    }
    if (bcast) {
        AlertDialog(
            onDismissRequest = { bcast = false },
            title = { Text("Broadcast") },
            text = { OutlinedTextField(bcastText, { bcastText = it.take(81) }, label = { Text("Message (81 characters)") }) },
            confirmButton = { TextButton({ Core.send("broadcast", "text" to bcastText); bcastText = ""; bcast = false }) { Text("Send") } },
            dismissButton = { TextButton({ bcast = false }) { Text("Cancel") } },
        )
    }
}

@Composable
fun Header(text: String) {
    Text(text, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp))
}

@Composable
fun Hint(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
}

// ---- VMail ----

private val folders = listOf("inbox" to "Inbox", "outbox" to "Outbox", "sent" to "Sent", "parking" to "Parked")

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun VMailScreen() {
    val revision by Core.vmailRevision.collectAsState()
    val partner by Core.partner.collectAsState()
    var folder by remember { mutableIntStateOf(0) }
    var open by remember { mutableStateOf<VMailItem?>(null) }
    var compose by remember { mutableStateOf<Pair<String, String>?>(null) }
    val items = remember(folder, revision, open) { Core.vmails(folders[folder].first) }

    val o = open
    if (o != null) {
        LaunchedEffect(o.guid) { if (!o.read) Core.markRead(o.guid) }
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp)) {
            Row {
                TextButton({ open = null }) { Text("‹ Back") }
                Spacer(Modifier.weight(1f))
                if (folder == 0) TextButton({ compose = o.from to (if (o.subject.startsWith("Re:")) o.subject else "Re: ${o.subject}") }) { Text("Reply") }
                TextButton({ Core.delete(o.guid); open = null }) { Text("Delete") }
            }
            Text(o.subject, style = MaterialTheme.typography.titleLarge)
            Text("From ${o.from} to ${o.to}", style = MaterialTheme.typography.bodyMedium)
            Text(o.time, style = MaterialTheme.typography.bodySmall)
            HorizontalDivider(Modifier.padding(vertical = 8.dp))
            Text(o.body)
        }
    } else {
        Column(Modifier.fillMaxSize()) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                SecondaryTabRow(folder, Modifier.weight(1f)) {
                    folders.forEachIndexed { i, (_, label) -> Tab(folder == i, { folder = i }, text = { Text(label) }) }
                }
                IconButton({ compose = (partner ?: "") to "" }) { Icon(Icons.Default.Edit, "New VMail") }
            }
            LazyColumn(Modifier.fillMaxSize()) {
                if (items.isEmpty()) item { Hint("No VMail here") }
                items(items) { m ->
                    Column(Modifier.fillMaxWidth().clickable { open = m }.padding(horizontal = 16.dp, vertical = 10.dp)) {
                        Row {
                            Text(
                                if (folder == 0 || folder == 3) m.from else m.to,
                                fontWeight = if (m.read) FontWeight.Normal else FontWeight.Bold, modifier = Modifier.weight(1f),
                            )
                            if (m.urgent) Pill("URGENT", Color(0xFFE5484D))
                            Text("  ${m.time}", style = MaterialTheme.typography.bodySmall)
                        }
                        Text(m.subject, maxLines = 1, style = MaterialTheme.typography.bodyMedium)
                    }
                    HorizontalDivider()
                }
            }
        }
    }
    compose?.let { (to, subject) -> ComposeDialog(to, subject) { compose = null } }
}

@Composable
fun ComposeDialog(to0: String, subject0: String, close: () -> Unit) {
    var to by remember { mutableStateOf(to0) }
    var subject by remember { mutableStateOf(subject0) }
    var body by remember { mutableStateOf("") }
    var urgent by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = close,
        title = { Text("New VMail") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                OutlinedTextField(to, { to = it.uppercase() }, label = { Text("To (call sign)") }, singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false))
                OutlinedTextField(subject, { subject = it }, label = { Text("Subject") }, singleLine = true)
                OutlinedTextField(body, { body = it }, label = { Text("Message") }, minLines = 4, maxLines = 10)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Switch(urgent, { urgent = it }); Text("  Urgent")
                }
                Text(
                    "It goes out now if you're in a QSO with them; otherwise it waits in the Outbox for your next QSO with them.",
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        },
        confirmButton = {
            TextButton({ Core.vmail(to, subject, body, urgent); close() }, enabled = to.isNotBlank() && body.isNotBlank()) { Text("Send") }
        },
        dismissButton = { TextButton(close) { Text("Cancel") } },
    )
}

// ---- settings ----

@Composable
fun SettingsScreen() {
    val context = LocalContext.current
    val prefs = remember { Prefs(context) }
    val status by Core.status.collectAsState()
    val log by Core.log.collectAsState()
    var call by remember { mutableStateOf(prefs.call) }
    var name by remember { mutableStateOf(prefs.name) }
    var qth by remember { mutableStateOf(prefs.qth) }
    var locator by remember { mutableStateOf(prefs.locator) }
    var frequency by remember { mutableStateOf(prefs.frequency.toString()) }
    var allowInfo by remember { mutableStateOf(prefs.allowInfo) }
    var allowPings by remember { mutableStateOf(prefs.allowPings) }
    var verboseSnrAuto by remember { mutableStateOf(prefs.verboseSnrAuto) }
    var skipSlotSelector by remember { mutableStateOf(prefs.skipSlotSelector) }
    var email by remember { mutableStateOf(prefs.email) }
    var fullCall by remember { mutableStateOf(prefs.fullCall) }
    var welcome by remember { mutableStateOf(prefs.welcome) }
    var welcomeAlways by remember { mutableStateOf(prefs.welcomeAlways) }
    var callIdMinutes by remember { mutableStateOf(prefs.callIdMinutes.toString()) }
    var idleMinutes by remember { mutableStateOf(prefs.idleMinutes.toString()) }
    var allowLastHeard by remember { mutableStateOf(prefs.allowLastHeard) }
    var multiBandLastHeard by remember { mutableStateOf(prefs.multiBandLastHeard) }
    var cqSlotWait by remember { mutableStateOf(prefs.cqSlotWait.toString()) }
    var showLog by remember { mutableStateOf(false) }
    fun save() {
        prefs.call = call.trim().uppercase(); prefs.name = name; prefs.qth = qth; prefs.locator = locator.uppercase()
        prefs.frequency = frequency.toLongOrNull() ?: 14_105_000
        Core.send("set_frequency", "hz" to prefs.frequency)
        Core.settingsSaved()
    }

    if (showLog) {
        Column(Modifier.fillMaxSize()) {
            TextButton({ showLog = false }) { Text("‹ Back") }
            LazyColumn(Modifier.fillMaxSize().padding(horizontal = 8.dp)) {
                items(log) { Text(it, fontFamily = FontFamily.Monospace, fontSize = 11.sp) }
            }
        }
        return
    }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Header("Station")
        OutlinedTextField(call, { call = it.uppercase() }, Modifier.fillMaxWidth(), label = { Text("Call sign") }, singleLine = true,
            keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Characters, autoCorrectEnabled = false))
        OutlinedTextField(name, { name = it }, Modifier.fillMaxWidth(), label = { Text("Name") }, singleLine = true)
        OutlinedTextField(qth, { qth = it }, Modifier.fillMaxWidth(), label = { Text("QTH") }, singleLine = true)
        OutlinedTextField(locator, { locator = it.uppercase() }, Modifier.fillMaxWidth(), label = { Text("Grid locator") }, singleLine = true)

        Header("Profile")
        OutlinedTextField(email, { email = it; prefs.email = it; Core.applyPolicy() }, Modifier.fillMaxWidth(),
            label = { Text("Email (shared in INFO)") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email))
        OutlinedTextField(fullCall, { fullCall = it.uppercase(); prefs.fullCall = fullCall; Core.applyPolicy() }, Modifier.fillMaxWidth(),
            label = { Text("Complex call sign, e.g. W/4Z1AC/QRP") }, singleLine = true)
        Text("Leave the email empty to keep it private. A complex call sign is sent in your opening (<FC:>); VARA links with the plain one.",
            style = MaterialTheme.typography.bodySmall)

        Header("Modem")
        Text("The modem (XARA on this phone or VARA on the network), its audio, PTT and drive, and the " +
            "Registered (VARA licence) switch are on the Modem tab.", style = MaterialTheme.typography.bodySmall)

        Header("QSO")
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(allowInfo, { allowInfo = it; prefs.allowInfo = it; Core.applyPolicy() }); Text("  Answer info requests")
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(allowPings, { allowPings = it; prefs.allowPings = it; Core.applyPolicy() }); Text("  Answer pings")
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(verboseSnrAuto, { verboseSnrAuto = it; prefs.verboseSnrAuto = it; Core.applyPolicy() }); Text("  Auto accept verbose SNR")
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(allowLastHeard, { allowLastHeard = it; prefs.allowLastHeard = it; Core.applyPolicy() }); Text("  Allow last heard peeking")
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(multiBandLastHeard, { multiBandLastHeard = it; prefs.multiBandLastHeard = it; Core.applyPolicy() }); Text("  Last heard on all bands")
        }
        OutlinedTextField(welcome, { welcome = it; prefs.welcome = it; Core.applyPolicy() }, Modifier.fillMaxWidth(),
            label = { Text("Welcome message") })
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(welcomeAlways, { welcomeAlways = it; prefs.welcomeAlways = it; Core.applyPolicy() }); Text("  Send the welcome even when away")
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedTextField(callIdMinutes, {
                callIdMinutes = it.filter(Char::isDigit)
                callIdMinutes.toIntOrNull()?.let { m -> prefs.callIdMinutes = m.coerceIn(5, 30); Core.applyPolicy() }
            }, Modifier.weight(1f), label = { Text("Call ID (min)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
            OutlinedTextField(idleMinutes, {
                idleMinutes = it.filter(Char::isDigit)
                idleMinutes.toIntOrNull()?.let { m -> prefs.idleMinutes = m.coerceIn(0, 60); Core.applyPolicy() }
            }, Modifier.weight(1f), label = { Text("Idle disconnect (min, 0 = never)") }, singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        }
        Text("With info requests off they're refused; with pings off the modem doesn't answer them. " +
            "The welcome goes to stations that call you (<LOC> is your locator).", style = MaterialTheme.typography.bodySmall)

        Header("CQ")
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(skipSlotSelector, { skipSlotSelector = it; prefs.skipSlotSelector = it }); Text("  Skip slot selector")
        }
        OutlinedTextField(cqSlotWait, {
            cqSlotWait = it.filter(Char::isDigit)
            cqSlotWait.toIntOrNull()?.let { s -> prefs.cqSlotWait = s.coerceIn(30, 900) }
        }, Modifier.fillMaxWidth(), label = { Text("Wait on the slot (s)") }, singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        Text("A CQ names the slot you'll wait on for replies, then you go back to the calling " +
            "frequency. With the selector skipped, CQs name no slot.", style = MaterialTheme.typography.bodySmall)

        Header("Radio")
        OutlinedTextField(frequency, { frequency = it.filter(Char::isDigit) }, Modifier.fillMaxWidth(), label = { Text("Dial frequency (Hz)") },
            singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        Text("For the QSO log (also on the Chat screen: tap the frequency); set your radio to it (USB). The phone doesn't tune the radio. " +
            "On a calling frequency only 500 Hz is allowed for CQ, connect, ping and beacon.", style = MaterialTheme.typography.bodySmall)

        Header("Status")
        Text(status)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button({ save(); Core.applySettings(context) }) { Text("Save") }
            OutlinedButton({ showLog = true }) { Text("Log") }
        }
        Text("Openslot ${Core.version}. Received files are in Android/data/io.github.nr5p.openslot/files/Received.", style = MaterialTheme.typography.bodySmall)
    }
}

@Composable
fun Choice(label: String, selected: Boolean, pick: () -> Unit) {
    Row(Modifier.clickable(onClick = pick).padding(end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        RadioButton(selected, pick); Text(label)
    }
}

@Composable
fun DevicePicker(label: String, devices: List<Device>, selected: Int, pick: (Int) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val current = devices.find { it.id == selected } ?: devices.firstOrNull { it.usb }
    Column {
        Text(label, style = MaterialTheme.typography.labelLarge)
        Box {
            OutlinedButton({ open = true }, Modifier.fillMaxWidth()) { Text(current?.label ?: "Choose…") }
            DropdownMenu(open, { open = false }) {
                devices.forEach { d -> DropdownMenuItem({ Text(d.label) }, { pick(d.id); open = false }) }
            }
        }
    }
}

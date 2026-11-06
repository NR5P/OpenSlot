package io.github.nr5p.openslot

import android.content.Context
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import android.provider.OpenableColumns
import android.text.format.Formatter
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Upload
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

/** A file picked to send, copied into the app's cache under its own name (the engine reads
 *  a path; a picked document is only a content URI). */
object Outgoing {
    private fun folder(context: Context): File =
        File(context.cacheDir, "Outgoing/${UUID.randomUUID()}").apply { mkdirs() }

    private fun displayName(context: Context, uri: Uri): String? =
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }

    /** A file from the system file picker, as it is. */
    suspend fun copyFile(context: Context, uri: Uri): String = withContext(Dispatchers.IO) {
        val name = displayName(context, uri)?.replace('/', '_')?.takeIf { it.isNotBlank() } ?: "file"
        val dest = File(folder(context), name)
        context.contentResolver.openInputStream(uri)?.use { input -> dest.outputStream().use { input.copyTo(it) } }
            ?: error("The file couldn't be read.")
        dest.path
    }

    /** A photo from the photo picker. JPEG/PNG/GIF/BMP go as they are; anything else (HEIC,
     *  WebP) becomes JPEG, which other stations can open and the shrinker can read, turned
     *  upright as the camera recorded it. */
    suspend fun copyPhoto(context: Context, uri: Uri): String = withContext(Dispatchers.IO) {
        val stamp = SimpleDateFormat("yyyyMMdd_HHmmss", Locale.US).format(Date())
        val ext = when (context.contentResolver.getType(uri)) {
            "image/jpeg" -> "jpg"
            "image/png" -> "png"
            "image/gif" -> "gif"
            "image/bmp", "image/x-ms-bmp" -> "bmp"
            else -> null
        }
        if (ext != null) {
            val dest = File(folder(context), "IMG_$stamp.$ext")
            context.contentResolver.openInputStream(uri)?.use { input -> dest.outputStream().use { input.copyTo(it) } }
                ?: error("The photo couldn't be read.")
            return@withContext dest.path
        }
        val bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(context.contentResolver, uri)) { d, _, _ ->
            d.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
        }
        val dest = File(folder(context), "IMG_$stamp.jpg")
        dest.outputStream().use { bitmap.compress(Bitmap.CompressFormat.JPEG, 90, it) }
        dest.path
    }
}

/** "about 3 min" / "40 s". */
fun spanText(secs: Double): String =
    if (secs < 60) "${maxOf(5, Math.round(secs / 5).toInt() * 5)} s" else "about ${Math.round(secs / 60)} min"

fun byteText(context: Context, n: Long): String = Formatter.formatShortFileSize(context, n)

/** The file going over the link, above the message box: waiting for the partner's answer,
 *  then how much is sent (or received); ours can be stopped. */
@Composable
fun TransferBar(t: FileTransfer, partner: String?) {
    val context = LocalContext.current
    var confirmStop by remember { mutableStateOf(false) }
    if (confirmStop) {
        AlertDialog(
            onDismissRequest = { confirmStop = false },
            title = { Text("Stop sending?") },
            text = { Text("The modem can't take back what it's already sending, so stopping ends the QSO with ${partner ?: "the station"}.") },
            confirmButton = { TextButton({ confirmStop = false; Core.cancelFile() }) { Text("Stop and end the QSO") } },
            dismissButton = { TextButton({ confirmStop = false }) { Text("Keep sending") } },
        )
    }
    Row(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(if (t.outgoing) Icons.Default.Upload else Icons.Default.Download, null, tint = MaterialTheme.colorScheme.primary)
        Spacer(Modifier.size(10.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(t.name, style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (t.total == 0L) {
                Text(
                    if (t.outgoing) "Waiting for ${partner ?: "the station"} to accept…" else "Receiving…",
                    style = MaterialTheme.typography.bodySmall,
                )
            } else {
                LinearProgressIndicator(progress = { t.fraction }, Modifier.fillMaxWidth())
                Text(
                    "${if (t.outgoing) "Sent" else "Received"} ${(t.fraction * 100).toInt()}% of ${byteText(context, t.total)}" +
                        (t.timeLeft?.let { " · ${spanText(it)} left" } ?: ""),
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        if (t.outgoing) {
            Spacer(Modifier.size(8.dp))
            // Withdrawing an offer is free; once the data is going only dropping the link stops it
            OutlinedButton({ if (t.total == 0L) Core.cancelFile() else confirmStop = true }) { Text("Stop") }
        }
    }
}

/** Before a file goes: its size, whether VARA allows it on this link and how long it takes.
 *  A picture goes through VarAC's image shrinker (resolution and quality), set at first to
 *  the largest size that fits, or as it is. */
@Composable
fun SendFileDialog(path: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val partner by Core.partner.collectAsState()
    val scope = rememberCoroutineScope()
    var info by remember { mutableStateOf<JSONObject?>(null) }
    var shrink by remember { mutableStateOf(true) }
    var pct by remember { mutableFloatStateOf(100f) }
    var quality by remember { mutableFloatStateOf(50f) }
    var working by remember { mutableStateOf(true) }

    fun load(asked: Int?) {
        working = true
        scope.launch {
            val i = Core.fileInfo(path, asked, quality.toInt())
            if (asked == null) {
                i.optJSONObject("shrunk")?.let { pct = it.optInt("pct", 100).toFloat() }
                // a picture that fits as it is goes as it is unless the operator shrinks it
                shrink = !i.optBoolean("fits")
            }
            info = i
            working = false
        }
    }
    LaunchedEffect(path) { load(null) }

    val i = info
    val isImage = i?.optBoolean("image") == true
    val shrunk = i?.optJSONObject("shrunk")
    val chosen = if (isImage && shrink) shrunk else i?.takeIf { !it.has("error") }
    val fits = chosen?.optBoolean("fits") == true
    fun size(d: JSONObject) = "${byteText(context, d.optLong("bytes"))}, ${byteText(context, d.optLong("compressed"))} compressed"

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Send a file") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                when {
                    i == null -> Row(verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(20.dp))
                        Text("  Reading the file…")
                    }
                    i.has("error") -> Text(i.optString("error"), color = MaterialTheme.colorScheme.error)
                    else -> {
                        Text(i.optString("name"), style = MaterialTheme.typography.titleSmall)
                        Text(size(i), style = MaterialTheme.typography.bodySmall)
                        if (isImage) Text("${i.optInt("w")} × ${i.optInt("h")}", style = MaterialTheme.typography.bodySmall)
                        if (!(isImage && shrink)) Text("Time: ${i.optString("estimate")}", style = MaterialTheme.typography.bodySmall)
                        if (isImage) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text("Shrink the picture", Modifier.weight(1f))
                                Switch(shrink, { shrink = it })
                            }
                            if (shrink) {
                                Text("Resolution ${pct.toInt()} %")
                                Slider(pct, { pct = Math.round(it).toFloat() }, valueRange = 1f..100f, onValueChangeFinished = { load(pct.toInt()) })
                                Text("Quality ${quality.toInt()}")
                                Slider(quality, { quality = it }, valueRange = 0f..50f, steps = 24, onValueChangeFinished = { load(pct.toInt()) })
                                if (shrunk != null) {
                                    Text("${shrunk.optInt("w")} × ${shrunk.optInt("h")}: ${size(shrunk)}", style = MaterialTheme.typography.bodySmall)
                                    Text("Time: ${shrunk.optString("estimate")}", style = MaterialTheme.typography.bodySmall)
                                } else if (i.has("shrink_error")) {
                                    Text(i.optString("shrink_error"), color = MaterialTheme.colorScheme.error)
                                }
                                Text(
                                    "As VarAC's image shrinker: the picture is scaled and sent as a JPEG.",
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                        }
                        if (chosen != null && !fits && !i.isNull("limit")) {
                            Text(
                                "VARA HF doesn't allow files over ${byteText(context, i.optLong("limit"))} compressed at ${Core.fileBandwidth} Hz." +
                                    if (isImage) " Shrink the picture more." else "",
                                color = MaterialTheme.colorScheme.error,
                            )
                        }
                        if (working) LinearProgressIndicator(Modifier.fillMaxWidth())
                    }
                }
            }
        },
        confirmButton = {
            TextButton(
                {
                    Core.sendFile(path, if (isImage && shrink) pct.toInt() else null, quality.toInt())
                    onDismiss()
                },
                enabled = !working && chosen != null && fits && partner != null,
            ) { Text("Send") }
        },
        dismissButton = { TextButton(onDismiss) { Text("Cancel") } },
    )
}

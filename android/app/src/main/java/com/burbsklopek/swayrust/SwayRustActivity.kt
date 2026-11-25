package com.burbsklopek.swayrust

import android.app.NativeActivity
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.ActivityInfo
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.KeyEvent
import android.view.Surface
import android.view.SurfaceHolder
import android.view.View
import java.io.File
import java.io.FileInputStream
import java.net.Inet4Address
import java.net.NetworkInterface

/**
 * Minimal NativeActivity host for Pure Native Rust Sway (OpenGL ES 3.0).
 * Zero WebView, zero Jetpack Compose overhead.
 */
class SwayRustActivity : NativeActivity() {

    private external fun nativeOnBackPressed()
    private external fun nativeOnMenuPressed()

    private var activeRefreshRateHz: Float = 60f

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        prefer4kHighRefreshDisplayMode()
        enterImmersiveMode()
    }

    override fun onResume() {
        super.onResume()
        prefer4kHighRefreshDisplayMode()
        enterImmersiveMode()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) enterImmersiveMode()
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        if (event.keyCode == KeyEvent.KEYCODE_BACK) {
            if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
                try {
                    nativeOnBackPressed()
                } catch (_: UnsatisfiedLinkError) {
                }
            }
            return true
        }
        if (event.keyCode == KeyEvent.KEYCODE_MENU) {
            if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
                try {
                    nativeOnMenuPressed()
                } catch (_: UnsatisfiedLinkError) {
                }
            }
            return true
        }
        return super.dispatchKeyEvent(event)
    }

    @Suppress("DEPRECATION")
    override fun onBackPressed() {
        try {
            nativeOnBackPressed()
        } catch (_: UnsatisfiedLinkError) {
        }
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        super.surfaceCreated(holder)
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val hz = activeRefreshRateHz.coerceIn(60f, 120f)
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                    holder.surface.setFrameRate(
                        hz,
                        Surface.FRAME_RATE_COMPATIBILITY_DEFAULT,
                        Surface.CHANGE_FRAME_RATE_ONLY_IF_SEAMLESS
                    )
                } else {
                    holder.surface.setFrameRate(hz, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT)
                }
            }
        } catch (_: Throwable) {
        }
    }

    private fun prefer4kHighRefreshDisplayMode() {
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                window.colorMode = ActivityInfo.COLOR_MODE_WIDE_COLOR_GAMUT
            }
            val d = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) display else @Suppress("DEPRECATION") windowManager.defaultDisplay
            val modes = d?.supportedModes
            val best = modes
                ?.filter { it.refreshRate <= 121f }
                ?.maxWithOrNull(compareBy({ it.physicalWidth * it.physicalHeight }, { it.refreshRate }))
                ?: modes?.maxWithOrNull(compareBy({ it.physicalWidth * it.physicalHeight }, { it.refreshRate }))
            if (best != null) {
                activeRefreshRateHz = best.refreshRate.coerceIn(60f, 120f)
                val lp = window.attributes
                lp.preferredDisplayModeId = best.modeId
                lp.preferredRefreshRate = activeRefreshRateHz
                window.attributes = lp
            }
        } catch (_: Throwable) {
        }
    }

    @Suppress("DEPRECATION")
    private fun enterImmersiveMode() {
        window.decorView.systemUiVisibility = (
            View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                or View.SYSTEM_UI_FLAG_FULLSCREEN
                or View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                or View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                or View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                or View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
            )
    }

    fun rustGetFilesDir(): String {
        return try {
            filesDir.absolutePath
        } catch (_: Throwable) {
            "/data/data/com.burbsklopek.swayrust/files"
        }
    }

    fun rustGetCacheDir(): String {
        return try {
            cacheDir.absolutePath
        } catch (_: Throwable) {
            "/data/data/com.burbsklopek.swayrust/cache"
        }
    }

    fun rustInstallApk(apkPath: String): String {
        return try {
            val file = File(apkPath)
            if (!file.exists() || file.length() < 100_000L) {
                return "ERR:Downloaded APK file is missing or incomplete"
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !packageManager.canRequestPackageInstalls()) {
                val permIntent = Intent(
                    Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                    Uri.parse("package:$packageName")
                ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                try {
                    startActivity(permIntent)
                } catch (_: Throwable) {
                }
                return "NEED_PERMISSION"
            }

            val installer = packageManager.packageInstaller
            val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
            params.setSize(file.length())
            val sessionId = installer.createSession(params)
            val session = installer.openSession(sessionId)

            session.openWrite("sway_rust_update", 0, file.length()).use { out ->
                FileInputStream(file).use { input ->
                    val buf = ByteArray(65_536)
                    while (true) {
                        val n = input.read(buf)
                        if (n <= 0) break
                        out.write(buf, 0, n)
                    }
                }
                session.fsync(out)
            }

            val callbackIntent = Intent(this, UpdateInstallReceiver::class.java)
            val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE
            } else {
                PendingIntent.FLAG_UPDATE_CURRENT
            }
            val pending = PendingIntent.getBroadcast(this, sessionId, callbackIntent, flags)
            session.commit(pending.intentSender)
            session.close()
            "OK"
        } catch (t: Throwable) {
            "ERR:${t.message ?: t.javaClass.simpleName}"
        }
    }

    fun rustGetLanIp(): String {
        return try {
            val interfaces = NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
            val prioritized = interfaces.sortedBy { nif ->
                val name = nif.name.lowercase()
                when {
                    name.startsWith("wlan") || name.startsWith("eth") || name.startsWith("en") -> 0
                    name.startsWith("tun") || name.startsWith("nord") || name.startsWith("wg") -> 2
                    else -> 1
                }
            }
            for (nif in prioritized) {
                if (!nif.isUp || nif.isLoopback) continue
                val name = nif.name.lowercase()
                if (name.startsWith("tun") || name.startsWith("nord") || name.startsWith("wg")) continue
                for (addr in nif.inetAddresses) {
                    if (!addr.isLoopbackAddress && addr is Inet4Address && addr.isSiteLocalAddress) {
                        return addr.hostAddress ?: ""
                    }
                }
            }
            ""
        } catch (_: Throwable) {
            ""
        }
    }

    fun rustLaunchTrailer(url: String, heightPx: Int, youtubeVq: String): Boolean {
        return try {
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                putExtra("preferred_video_quality", heightPx)
                putExtra("vq", youtubeVq)
                putExtra("force_fullscreen", true)
            }
            startActivity(intent)
            true
        } catch (_: Throwable) {
            false
        }
    }

    fun rustExitApp() {
        try {
            finish()
        } catch (_: Throwable) {
        }
    }
}

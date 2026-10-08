package dev.lancast.receiver

import android.app.Activity
import android.widget.FrameLayout
import android.widget.LinearLayout

/** These APKs have no AirPlay service, native library or enabled control. */
object AirPlayFeatureFactory {
    @Suppress("UNUSED_PARAMETER")
    fun create(activity: Activity, controls: LinearLayout, display: FrameLayout): AirPlayFeature = object : AirPlayFeature {
        override fun onStart() {}
        override fun onStop() {}
        override fun close() {}
        override fun stopCurrent() = false
        override fun refreshDisplay() {}
    }
}

package dev.lancast.receiver

import android.app.Activity
import android.widget.FrameLayout
import android.widget.LinearLayout
import dev.lancast.airplay.AirPlayPanel

object AirPlayFeatureFactory {
    fun create(activity: Activity, controls: LinearLayout, display: FrameLayout): AirPlayFeature {
        val panel = AirPlayPanel(activity, controls, display)
        return object : AirPlayFeature {
            override fun onStart() = panel.onStart()
            override fun onStop() = panel.onStop()
            override fun close() = panel.close()
            override fun stopCurrent() = panel.stopCurrent()
            override fun refreshDisplay() = panel.refreshDisplay()
        }
    }
}

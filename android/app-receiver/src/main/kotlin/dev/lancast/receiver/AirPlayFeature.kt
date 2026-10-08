package dev.lancast.receiver

interface AirPlayFeature {
    fun onStart()
    fun onStop()
    fun close()
    fun stopCurrent(): Boolean
    fun refreshDisplay()
}

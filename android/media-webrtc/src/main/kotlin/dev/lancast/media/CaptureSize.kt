package dev.lancast.media

/** Resize callbacks describe source pixels; they cannot enlarge the negotiated encoder budget. */
internal fun fitCaptureSize(width: Int, height: Int, maxWidth: Int, maxHeight: Int): Pair<Int, Int> {
    require(width > 0 && height > 0 && maxWidth >= 2 && maxHeight >= 2)
    val scale = minOf(1.0, maxWidth.toDouble() / width, maxHeight.toDouble() / height)
    fun even(value: Int): Int = ((value * scale).toInt().coerceAtLeast(2) / 2) * 2
    return even(width) to even(height)
}

package dev.lancast.sender

import android.opengl.EGL14
import android.opengl.EGLExt
import android.opengl.GLES20
import android.view.Surface

/** Synthetic encoder input. It has no projection, screen, microphone or playback capture handle. */
class ProbeFrames(surface: Surface) : AutoCloseable {
    private val display = EGL14.eglGetDisplay(EGL14.EGL_DEFAULT_DISPLAY)
    private var context = EGL14.EGL_NO_CONTEXT
    private var window = EGL14.EGL_NO_SURFACE
    init {
        try {
            check(EGL14.eglInitialize(display, IntArray(2), 0, IntArray(2), 0))
            val configs = arrayOfNulls<android.opengl.EGLConfig>(1)
            val attributes = intArrayOf(EGL14.EGL_RED_SIZE, 8, EGL14.EGL_GREEN_SIZE, 8, EGL14.EGL_BLUE_SIZE, 8,
                EGL14.EGL_RENDERABLE_TYPE, EGL14.EGL_OPENGL_ES2_BIT, 0x3142, 1, EGL14.EGL_NONE)
            check(EGL14.eglChooseConfig(display, attributes, 0, configs, 0, 1, IntArray(1), 0))
            context = EGL14.eglCreateContext(display, configs[0], EGL14.EGL_NO_CONTEXT, intArrayOf(EGL14.EGL_CONTEXT_CLIENT_VERSION, 2, EGL14.EGL_NONE), 0)
            check(context != EGL14.EGL_NO_CONTEXT)
            window = EGL14.eglCreateWindowSurface(display, configs[0], surface, intArrayOf(EGL14.EGL_NONE), 0)
            check(window != EGL14.EGL_NO_SURFACE)
            check(EGL14.eglMakeCurrent(display, window, window, context))
        } catch (e: Exception) { close(); throw e }
    }
    fun draw(frame: Long, timestampNs: Long) {
        val phase = frame / 30 % 3
        GLES20.glClearColor(if (phase == 0L) 1f else 0f, if (phase == 1L) 1f else 0f, if (phase == 2L) 1f else 0f, 1f)
        GLES20.glClear(GLES20.GL_COLOR_BUFFER_BIT)
        check(EGLExt.eglPresentationTimeANDROID(display, window, timestampNs))
        check(EGL14.eglSwapBuffers(display, window))
    }
    override fun close() {
        EGL14.eglMakeCurrent(display, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_SURFACE, EGL14.EGL_NO_CONTEXT)
        if (window != EGL14.EGL_NO_SURFACE) EGL14.eglDestroySurface(display, window)
        if (context != EGL14.EGL_NO_CONTEXT) EGL14.eglDestroyContext(display, context)
        EGL14.eglReleaseThread(); EGL14.eglTerminate(display)
        window = EGL14.EGL_NO_SURFACE; context = EGL14.EGL_NO_CONTEXT
    }
}

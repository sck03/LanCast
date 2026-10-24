package dev.lancast.receiver.contracts

import java.util.concurrent.atomic.AtomicBoolean

/** Resource owners report their actual exit. Cancellation alone never transfers ownership. */
class WorkerCompletion(private val workers: Int) {
    init { require(workers > 0) }
    private val lock = Any()
    private var remaining = workers
    private var issued = 0
    private val callbacks = mutableListOf<() -> Unit>()
    fun participant(): () -> Unit {
        synchronized(lock) { check(issued < workers); issued++ }
        val finished = AtomicBoolean(false)
        return {
            if (finished.compareAndSet(false, true)) {
                val ready = synchronized(lock) {
                    remaining--
                    if (remaining == 0) callbacks.toList().also { callbacks.clear() } else emptyList()
                }
                ready.forEach { it() }
            }
        }
    }
    fun whenComplete(callback: () -> Unit) {
        val done = synchronized(lock) {
            if (remaining == 0) true else { callbacks += callback; false }
        }
        if (done) callback()
    }
}

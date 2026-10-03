#include "lancast_media.h"
#include <jni.h>
#include <map>
#include <memory>
#include <mutex>
#include <vector>
struct Bridge {
    LcTsMux* mux = nullptr; JNIEnv* env = nullptr; jobject sink = nullptr; jmethodID method = nullptr;
};
static std::mutex registry_mutex;
static std::map<jlong, std::unique_ptr<Bridge>> registry;
static jlong next_handle = 1;
static int32_t output(void* opaque, const uint8_t* bytes, size_t size) {
    auto& b = *static_cast<Bridge*>(opaque);
    jbyteArray array = b.env->NewByteArray(static_cast<jsize>(size)); if (!array) return -1;
    b.env->SetByteArrayRegion(array, 0, static_cast<jsize>(size), reinterpret_cast<const jbyte*>(bytes));
    jint result = b.env->CallIntMethod(b.sink, b.method, array);
    b.env->DeleteLocalRef(array);
    if (b.env->ExceptionCheck()) { b.env->ExceptionClear(); return -1; }
    return result;
}
extern "C" JNIEXPORT jlong JNICALL Java_dev_lancast_sender_TsMux_create(JNIEnv* env, jobject, jint width, jint height, jboolean audio, jobject sink) {
    try {
        auto b = std::make_unique<Bridge>(); b->env = env; b->sink = env->NewGlobalRef(sink);
        auto cls = env->GetObjectClass(sink); b->method = env->GetMethodID(cls, "onTs", "([B)I"); env->DeleteLocalRef(cls);
        if (!b->sink || !b->method) { if (b->sink) env->DeleteGlobalRef(b->sink); return 0; }
        const uint8_t asc[] = {0x11, 0x90};
        LcTsConfig c{}; c.size=sizeof(c); c.abi_version=1; c.width=width; c.height=height; c.fps=30; c.video_bitrate=3000000;
        c.audio_rate=48000; c.audio_channels=audio ? 2 : 0; c.audio_specific_config=asc; c.audio_specific_config_size=2; c.write=output; c.user=b.get();
        if (lc_ts_create(&c, &b->mux) != 0) { env->DeleteGlobalRef(b->sink); return 0; }
        std::lock_guard lock(registry_mutex); auto id=next_handle++; registry.emplace(id,std::move(b)); return id;
    } catch (...) { return 0; }
}
extern "C" JNIEXPORT jint JNICALL Java_dev_lancast_sender_TsMux_write(JNIEnv* env, jobject, jlong id, jboolean audio, jbyteArray data, jlong pts) {
    try {
        std::lock_guard lock(registry_mutex); auto it=registry.find(id); if(it==registry.end() || !data) return -1;
        auto& b=*it->second; b.env=env;
        auto n=env->GetArrayLength(data); if(n<=0 || n>8*1024*1024) return -1;
        std::vector<uint8_t> bytes(n); env->GetByteArrayRegion(data,0,n,reinterpret_cast<jbyte*>(bytes.data()));
        if(env->ExceptionCheck()) return -1;
        return audio ? lc_ts_audio(b.mux,bytes.data(),bytes.size(),pts) : lc_ts_video(b.mux,bytes.data(),bytes.size(),pts);
    } catch (...) { return -1; }
}
extern "C" JNIEXPORT void JNICALL Java_dev_lancast_sender_TsMux_destroy(JNIEnv* env, jobject, jlong id) {
    std::lock_guard lock(registry_mutex); auto it=registry.find(id); if(it==registry.end()) return;
    it->second->env=env; lc_ts_destroy(it->second->mux); env->DeleteGlobalRef(it->second->sink); registry.erase(it);
}

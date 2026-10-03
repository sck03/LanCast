plugins { id("com.android.library"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.lancast.media"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    defaultConfig { minSdk = 21 }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
}
kotlin { jvmToolchain(17) }
dependencies { coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
    implementation("androidx.annotation:annotation:1.9.1")
    // Explicit build input; never silently pick up a different local AAR.
    val sourceAar = providers.gradleProperty("lancastWebrtcAar").orNull
    if (sourceAar != null) {
        require(file(sourceAar).isFile) { "lancastWebrtcAar does not exist" }
        api(files(sourceAar))
    } else {
        // Transitional artifact, tracked in ADR D07; custom-source release is gated separately.
        api("io.github.webrtc-sdk:android:150.7871.01")
    }
}

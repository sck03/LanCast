plugins { alias(libs.plugins.android.library) }
android {
    namespace = "dev.lancast.media"
    compileSdk = libs.versions.compileSdk.get().toInt()
    ndkVersion = libs.versions.ndk.get()
    buildToolsVersion = libs.versions.buildTools.get()
    defaultConfig { minSdk = 21; consumerProguardFiles("consumer-rules.pro") }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
}
dependencies { coreLibraryDesugaring(libs.desugar)
    testImplementation("junit:junit:4.13.2")
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

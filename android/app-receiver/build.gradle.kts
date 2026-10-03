plugins { id("com.android.application"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.lancast.receiver"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    defaultConfig {
        applicationId = "dev.lancast.receiver"
        minSdk = 21; targetSdk = 36; versionCode = 2; versionName = "0.2.0-alpha"
        ndk { abiFilters += listOf("armeabi-v7a", "arm64-v8a") }
    }
    buildFeatures { buildConfig = true }
    flavorDimensions += "support"
    productFlavors {
        create("standard") { dimension = "support"; minSdk = 23 }
        create("legacy") { dimension = "support"; minSdk = 21; applicationIdSuffix = ".legacy" }
    }
    buildTypes { release { isMinifyEnabled = false } }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
    packaging { jniLibs { useLegacyPackaging = false } }
}
kotlin { jvmToolchain(17) }
dependencies {
    implementation(project(":control-bridge"))
    implementation(project(":media-webrtc"))
    implementation(project(":player-api"))
    add("standardImplementation", project(":player-standard"))
    add("legacyImplementation", project(":player-legacy"))
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
}

plugins { id("com.android.library"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.lancast.control"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    defaultConfig { minSdk = 21 }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
}
kotlin { jvmToolchain(17) }
dependencies { coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
    api("com.squareup.okhttp3:okhttp:4.12.0")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}

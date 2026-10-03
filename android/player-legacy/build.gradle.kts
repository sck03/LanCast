plugins { id("com.android.library"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.lancast.player.legacy"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    defaultConfig { minSdk = 21 }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17; isCoreLibraryDesugaringEnabled = true }
}
kotlin { jvmToolchain(17) }
dependencies { coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
    implementation(project(":player-api"))
    implementation(project(":control-bridge"))
    implementation("androidx.media3:media3-exoplayer:1.8.1")
    implementation("androidx.media3:media3-ui:1.8.1")
    implementation("androidx.media3:media3-datasource-okhttp:1.8.1")
}
android.sourceSets["main"].java.srcDir("../player-media3/src/main/kotlin")

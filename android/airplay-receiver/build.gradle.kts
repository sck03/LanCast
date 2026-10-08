plugins { id("com.android.library"); id("org.jetbrains.kotlin.android") }
android {
    namespace = "dev.lancast.airplay"
    compileSdk = 36
    defaultConfig { minSdk = 23 }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    consumerProguardFiles("consumer-rules.pro")
}
kotlin { jvmToolchain(17) }
dependencies { implementation(project(":receiver-contracts")) }

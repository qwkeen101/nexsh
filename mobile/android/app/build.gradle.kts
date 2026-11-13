plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.nexsh.mobile"
    compileSdk = 35

    defaultConfig {
        applicationId = "dev.nexsh.mobile"
        minSdk = 26
        targetSdk = 35
        // 版本号由 CI 按标签传入；本地构建时是开发版。
        versionCode = (System.getenv("NEXSH_VERSION_CODE") ?: "1").toInt()
        versionName = System.getenv("NEXSH_VERSION") ?: "dev"
    }

    buildFeatures {
        compose = true
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.core:core-ktx:1.15.0")
}

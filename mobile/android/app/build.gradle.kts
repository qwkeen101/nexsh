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

    // 发布包用固定的签名密钥，这样新版本可以直接覆盖安装。密钥库不在仓库里：
    // CI 从仓库的 Secret 里取出来放到 NEXSH_KEYSTORE 指的位置。没有它时（本地构建、
    // 没配 Secret 的仓库）退回调试签名，包照样能装，只是不能覆盖别的机器打出来的包。
    val keystore = System.getenv("NEXSH_KEYSTORE").orEmpty()
    signingConfigs {
        if (keystore.isNotEmpty()) {
            create("fixed") {
                storeFile = file(keystore)
                storeType = "pkcs12"
                storePassword = System.getenv("NEXSH_KEYSTORE_PASSWORD")
                keyAlias = "nexsh"
                keyPassword = System.getenv("NEXSH_KEYSTORE_PASSWORD")
            }
        }
    }
    buildTypes {
        release {
            // 代码量很小，不做混淆和裁剪：JNI 按名字找 Kotlin 一侧的函数，裁剪只会添乱。
            isMinifyEnabled = false
            signingConfig = signingConfigs.findByName("fixed") ?: signingConfigs.getByName("debug")
        }
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

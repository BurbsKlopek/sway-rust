plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val userHome: String = System.getProperty("user.home") ?: ""
val signingDirCandidates = listOf(
    File("$userHome/Documents/Android Signing"),
    File("$userHome/Documents/Android Development/Android Signing")
)
val activeSigningDir = signingDirCandidates.firstOrNull {
    File(it, "burbsklopek.jks").isFile && File(it, "burbsklopek.jks.pass").isFile
}

android {
    namespace = "com.burbsklopek.swayrust"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.burbsklopek.swayrust"
        minSdk = 26
        targetSdk = 35
        versionCode = 2
        versionName = "1.1.0"

        ndk {
            abiFilters += listOf("armeabi-v7a", "arm64-v8a")
        }
    }

    signingConfigs {
        if (activeSigningDir != null) {
            create("burbsRelease") {
                storeFile = File(activeSigningDir, "burbsklopek.jks")
                val pass = File(activeSigningDir, "burbsklopek.jks.pass").readText().trim()
                storePassword = pass
                keyAlias = "burbsklopek"
                keyPassword = pass
                enableV1Signing = true
                enableV2Signing = true
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro"
            )
            signingConfig = if (activeSigningDir != null) {
                signingConfigs.getByName("burbsRelease")
            } else {
                signingConfigs.getByName("debug")
            }
        }
        debug {
            isDebuggable = true
        }
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
    // Pure Native Rust OpenGL ES 3.0 build: zero WebView, zero Compose/AppCompat overhead
}

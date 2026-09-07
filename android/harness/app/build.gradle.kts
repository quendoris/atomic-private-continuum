plugins {
    id("com.android.application")
}

android {
    namespace = "org.atomicprivatecontinuum.harness"
    compileSdk = 37
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "org.atomicprivatecontinuum.harness"
        minSdk = 28
        targetSdk = 37
        versionCode = 1
        versionName = "0.0.1"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
}

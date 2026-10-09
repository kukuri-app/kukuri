buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("com.android.tools.build:gradle:9.3.1")
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.10")
    }
}

allprojects {
    repositories {
        google()
        mavenCentral()
    }
}

// Tauri の Android library と plugin の project は cargo の registry の中にある。その build の出力は、
// tauri-plugin の build script が directory ごと写す対象に入り、Gradle の書込みと競合するため、この project の下へ出す。
subprojects {
    if (name != "app") {
        layout.buildDirectory.set(rootProject.layout.buildDirectory.dir(name))
    }
}

tasks.register("clean").configure {
    delete("build")
}

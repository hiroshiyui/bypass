// SPDX-License-Identifier: GPL-3.0-or-later

pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        // OpenKeychain's openpgp-api library isn't on Maven Central
        // — only on JitPack. Scope to OpenKeychain's group so we
        // don't accidentally resolve anything else from JitPack.
        maven {
            url = uri("https://jitpack.io")
            content {
                includeGroup("com.github.open-keychain")
            }
        }
    }
}

rootProject.name = "bypass-android"
include(":app")

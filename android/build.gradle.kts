// SPDX-License-Identifier: GPL-3.0-or-later
//
// Root build script. The actual Android module config lives in
// `app/build.gradle.kts`; this file just declares the plugins
// the project uses so each subproject can `apply` them by id.

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.android) apply false
    alias(libs.plugins.kotlin.compose) apply false
}

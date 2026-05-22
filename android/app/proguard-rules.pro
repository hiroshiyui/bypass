# SPDX-License-Identifier: GPL-3.0-or-later
#
# ProGuard rules. AGP's default release profile keeps minification
# off (`isMinifyEnabled = false` in build.gradle.kts), so this file
# is currently a no-op. Kept on disk so a future release that flips
# minify on has a place to add Compose / UniFFI-specific keep rules.
#
# Common rules that will probably be needed when minify lands:
#   -keep class uniffi.bypass.** { *; }    # UniFFI uses JNI; keep
#   -keep class * implements uniffi.bypass.Crypto { *; }  # callbacks

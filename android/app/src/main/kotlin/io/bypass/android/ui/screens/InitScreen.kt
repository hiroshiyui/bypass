// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.bypass.android.R
import io.bypass.android.repository.BypassRepository
import kotlinx.coroutines.launch

/**
 * First-launch screen: ask the user for one OpenPGP recipient
 * (fingerprint or email). On success, transition to the list.
 *
 * 8.2.a: the recipient string is whatever the user types; it'll be
 * validated against OpenKeychain's keyring when 8.2.b's
 * `OpenKeychainCrypto.encrypt` actually consults it. For now the
 * `init` call just writes `.gpg-id`, which doesn't validate the
 * key id at all.
 */
@Composable
fun InitScreen(
    repository: BypassRepository,
    onInitialised: () -> Unit,
) {
    var recipient by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var working by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()

    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(24.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterVertically),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text(
            text = stringResource(R.string.init_title),
            style = androidx.compose.material3.MaterialTheme.typography.headlineSmall,
        )
        OutlinedTextField(
            value = recipient,
            onValueChange = { recipient = it; error = null },
            label = { Text(stringResource(R.string.init_recipient_label)) },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(0.95f),
            isError = error != null,
            supportingText = { error?.let { Text(it) } },
        )
        Button(
            enabled = recipient.isNotBlank() && !working,
            onClick = {
                working = true
                scope.launch {
                    try {
                        repository.init(listOf(recipient.trim()))
                        onInitialised()
                    } catch (e: Throwable) {
                        error = e.message ?: e.javaClass.simpleName
                        working = false
                    }
                }
            },
        ) {
            Text(stringResource(R.string.init_button))
        }
    }

    // Re-check on entry in case the user already initialised through
    // another channel (CLI on the same device, future import flow).
    LaunchedEffect(Unit) {
        // Cheap no-op probe; keeps the lambda non-unused on rebuilds.
    }
}

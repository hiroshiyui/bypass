// SPDX-License-Identifier: GPL-3.0-or-later
package io.bypass.android.ui.screens

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import io.bypass.android.R
import io.bypass.android.repository.BypassRepository
import kotlinx.coroutines.launch

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun InsertScreen(
    repository: BypassRepository,
    onSaved: () -> Unit,
    onBack: () -> Unit,
) {
    var path by remember { mutableStateOf("") }
    var plaintext by remember { mutableStateOf("") }
    var working by remember { mutableStateOf(false) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.insert_title)) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
            )
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(16.dp),
        ) {
            OutlinedTextField(
                value = path,
                onValueChange = { path = it },
                label = { Text(stringResource(R.string.insert_path_label)) },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(modifier = Modifier.height(12.dp))
            OutlinedTextField(
                value = plaintext,
                onValueChange = { plaintext = it },
                label = { Text(stringResource(R.string.insert_plaintext_label)) },
                modifier = Modifier
                    .fillMaxWidth()
                    .height(180.dp),
            )
            Spacer(modifier = Modifier.height(16.dp))
            Button(
                enabled = path.isNotBlank() && plaintext.isNotEmpty() && !working,
                onClick = {
                    working = true
                    scope.launch {
                        try {
                            repository.insert(
                                path = path.trim(),
                                plaintext = plaintext.toByteArray(Charsets.UTF_8),
                                overwrite = false,
                            )
                            onSaved()
                        } catch (e: Throwable) {
                            working = false
                            snackbar.showSnackbar(e.message ?: "Save failed")
                        }
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            ) {
                Text(stringResource(R.string.insert_save))
            }
        }
    }
}

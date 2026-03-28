<script lang="ts">
	import { onMount, tick } from 'svelte';
	import { chatStore, isStreaming } from '$lib/stores/chat';
	import { pendingChanges, selectedFile } from '$lib/stores/workspace';
	import { sendMessage, onStreamToken, getSettings } from '$lib/tauri';
	import ChatMessage from '$lib/components/ChatMessage.svelte';
	import ChatInput from '$lib/components/ChatInput.svelte';
	import Sidebar from '$lib/components/Sidebar.svelte';
	import Settings from '$lib/components/Settings.svelte';
	import FilePreview from '$lib/components/FilePreview.svelte';
	import DiffView from '$lib/components/DiffView.svelte';
	import SkillGallery from '$lib/components/SkillGallery.svelte';

	let showSettings = false;
	let needsApiKey = false;
	let messagesEl: HTMLElement;
	let streamingId: string | null = null;

	// Active panel: 'chat' | 'preview' | 'changes' | 'skills'
	let activePanel: 'chat' | 'preview' | 'changes' | 'skills' = 'chat';

	// Switch to preview when a file is selected
	$: if ($selectedFile) {
		activePanel = 'preview';
	}

	// Switch to changes when there are pending changes
	$: if ($pendingChanges.length > 0 && activePanel === 'chat' && $chatStore.length === 0) {
		activePanel = 'changes';
	}

	onMount(async () => {
		try {
			const key = await getSettings();
			if (!key) {
				needsApiKey = true;
				showSettings = true;
			}
		} catch (e) {
			// tauri not available in browser preview
		}

		try {
			const unlisten = await onStreamToken((token) => {
				if (streamingId) {
					chatStore.appendToMessage(streamingId, token);
					scrollToBottom();
				}
			});
			// Clean up on page unload since Svelte 5 onMount doesn't support async cleanup
			window.addEventListener('beforeunload', () => unlisten());
		} catch (e) {
			// onStreamToken not available outside Tauri
		}
	});

	async function scrollToBottom() {
		await tick();
		if (messagesEl) {
			messagesEl.scrollTop = messagesEl.scrollHeight;
		}
	}

	async function handleSend(e: CustomEvent<string>) {
		const message = e.detail;
		if ($isStreaming) return;

		activePanel = 'chat';
		chatStore.addMessage('user', message);
		await scrollToBottom();

		isStreaming.set(true);
		streamingId = chatStore.addMessage('assistant', '');
		await scrollToBottom();

		try {
			const result = await sendMessage(message);
			chatStore.updateMessage(streamingId, result);
		} catch (err) {
			const errMsg = err instanceof Error ? err.message : String(err);
			chatStore.updateMessage(streamingId, errMsg);
		} finally {
			streamingId = null;
			isStreaming.set(false);
			await scrollToBottom();
		}
	}

	function handleNewChat() {
		chatStore.clear();
		activePanel = 'chat';
	}

	$: pendingCount = $pendingChanges.filter((c) => c.status === 'Pending').length;
</script>

<div class="app">
	<Sidebar
		showSettings={showSettings}
		on:openSettings={() => (showSettings = true)}
		on:newChat={handleNewChat}
		on:openSkills={() => (activePanel = 'skills')}
	/>

	<div class="main">
		<!-- Tab bar -->
		<div class="tab-bar">
			<button
				class="tab"
				class:active={activePanel === 'chat'}
				on:click={() => (activePanel = 'chat')}
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="13" height="13">
					<path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
				</svg>
				Chat
			</button>
			<button
				class="tab"
				class:active={activePanel === 'preview'}
				on:click={() => (activePanel = 'preview')}
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="13" height="13">
					<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path>
					<polyline points="14 2 14 8 20 8"></polyline>
				</svg>
				File Preview
			</button>
			<button
				class="tab"
				class:active={activePanel === 'changes'}
				on:click={() => (activePanel = 'changes')}
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="13" height="13">
					<polyline points="16 3 21 3 21 8"></polyline>
					<line x1="4" y1="20" x2="21" y2="3"></line>
					<polyline points="21 16 21 21 16 21"></polyline>
					<line x1="15" y1="15" x2="21" y2="21"></line>
				</svg>
				Changes
				{#if pendingCount > 0}
					<span class="badge">{pendingCount}</span>
				{/if}
			</button>
			<button
				class="tab"
				class:active={activePanel === 'skills'}
				on:click={() => (activePanel = 'skills')}
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" width="13" height="13">
					<path d="M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z"></path>
					<polyline points="3.27 6.96 12 12.01 20.73 6.96"></polyline>
					<line x1="12" y1="22.08" x2="12" y2="12"></line>
				</svg>
				Skills
			</button>
		</div>

		<!-- Panel content -->
		<div class="panel" class:visible={activePanel === 'chat'}>
			{#if $chatStore.length === 0}
				<div class="empty-state">
					<div class="empty-icon">◉</div>
					<h1>How can I help you?</h1>
					<p>I'm KittyPaw, your AI agent. I can run code, automate tasks, and answer questions.</p>
					{#if needsApiKey}
						<div class="api-key-notice">
							<strong>Set up your API key</strong> in Settings to get started.
						</div>
					{/if}
				</div>
			{:else}
				<div class="messages" bind:this={messagesEl}>
					{#each $chatStore as message (message.id)}
						<ChatMessage {message} />
					{/each}
				</div>
			{/if}
			<ChatInput disabled={$isStreaming} on:send={handleSend} />
		</div>

		<div class="panel" class:visible={activePanel === 'preview'}>
			<FilePreview />
		</div>

		<div class="panel changes-panel" class:visible={activePanel === 'changes'}>
			{#if $pendingChanges.length === 0}
				<div class="empty-state">
					<div class="empty-icon">✓</div>
					<h1>No pending changes</h1>
					<p>File changes proposed by KittyPaw will appear here for review.</p>
				</div>
			{:else}
				<div class="changes-list">
					{#each $pendingChanges as change (change.id)}
						<DiffView {change} />
					{/each}
				</div>
			{/if}
		</div>

		<div class="panel" class:visible={activePanel === 'skills'}>
			<SkillGallery />
		</div>
	</div>
</div>

{#if showSettings}
	<Settings on:close={() => (showSettings = false)} />
{/if}

<style>
	.app {
		display: flex;
		height: 100vh;
		overflow: hidden;
	}

	.main {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		background: #fff;
	}

	/* Tab bar */
	.tab-bar {
		display: flex;
		align-items: center;
		gap: 2px;
		padding: 8px 12px 0;
		background: #f8fafc;
		border-bottom: 1px solid #e2e8f0;
	}

	.tab {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 7px 14px;
		border: none;
		background: transparent;
		color: #64748b;
		font-size: 13px;
		font-weight: 500;
		cursor: pointer;
		border-radius: 7px 7px 0 0;
		transition: background 0.12s, color 0.12s;
		position: relative;
	}

	.tab:hover {
		background: #e2e8f0;
		color: #334155;
	}

	.tab.active {
		background: #fff;
		color: #1e293b;
		border: 1px solid #e2e8f0;
		border-bottom: 1px solid #fff;
		margin-bottom: -1px;
	}

	.badge {
		background: #ef4444;
		color: #fff;
		font-size: 10px;
		font-weight: 700;
		padding: 1px 5px;
		border-radius: 10px;
		min-width: 16px;
		text-align: center;
	}

	/* Panels */
	.panel {
		display: none;
		flex: 1;
		flex-direction: column;
		overflow: hidden;
	}

	.panel.visible {
		display: flex;
	}

	.changes-panel.visible {
		overflow-y: auto;
	}

	/* Empty state */
	.empty-state {
		flex: 1;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		text-align: center;
		padding: 40px;
	}

	.empty-icon {
		font-size: 48px;
		color: #3b82f6;
		margin-bottom: 16px;
	}

	.empty-state h1 {
		font-size: 24px;
		font-weight: 600;
		color: #1e293b;
		margin: 0 0 10px;
	}

	.empty-state p {
		font-size: 15px;
		color: #64748b;
		max-width: 380px;
		margin: 0 0 20px;
	}

	.api-key-notice {
		background: #fef9c3;
		border: 1px solid #fde047;
		border-radius: 10px;
		padding: 12px 20px;
		font-size: 13px;
		color: #854d0e;
	}

	/* Messages */
	.messages {
		flex: 1;
		overflow-y: auto;
		padding: 20px 24px;
		scroll-behavior: smooth;
	}

	/* Changes list */
	.changes-list {
		padding: 16px 20px;
		display: flex;
		flex-direction: column;
	}
</style>

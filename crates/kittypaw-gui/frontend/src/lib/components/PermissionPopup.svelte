<script lang="ts">
	import { onMount } from 'svelte';
	import { pendingPermissionRequest } from '$lib/stores/permission';
	import { onPermissionRequest, respondPermissionRequest } from '$lib/tauri';

	onMount(() => {
		let unlisten: (() => void) | null = null;

		onPermissionRequest((req) => {
			pendingPermissionRequest.set(req);
		}).then((fn) => {
			unlisten = fn;
		}).catch(() => {
			// onPermissionRequest not available outside Tauri
		});

		return () => {
			unlisten?.();
		};
	});

	async function respond(decision: 'allow_once' | 'allow_permanent' | 'deny') {
		const req = $pendingPermissionRequest;
		if (!req) return;
		pendingPermissionRequest.set(null);
		try {
			await respondPermissionRequest(req.request_id, decision);
		} catch (e) {
			console.error('Failed to respond to permission request:', e);
		}
	}

	$: req = $pendingPermissionRequest;
</script>

{#if req}
	<div class="overlay" role="dialog" aria-modal="true" aria-label="Permission Request">
		<div class="popup">
			<div class="popup-header">
				<span class="icon">{req.resource_kind === 'file' ? '📄' : '🌐'}</span>
				<h3>Permission Required</h3>
			</div>

			<div class="detail">
				<div class="row">
					<span class="label">Resource</span>
					<span class="value mono">{req.resource_path}</span>
				</div>
				<div class="row">
					<span class="label">Action</span>
					<span class="value">{req.action}</span>
				</div>
				<div class="row">
					<span class="label">Workspace</span>
					<span class="value mono">{req.workspace_id}</span>
				</div>
			</div>

			<div class="actions">
				<button class="btn deny" on:click={() => respond('deny')}>거부</button>
				<button class="btn once" on:click={() => respond('allow_once')}>이번만 허용</button>
				<button class="btn permanent" on:click={() => respond('allow_permanent')}>영구 허용</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.overlay {
		position: fixed;
		inset: 0;
		background: rgba(0, 0, 0, 0.5);
		display: flex;
		align-items: center;
		justify-content: center;
		z-index: 1000;
	}

	.popup {
		background: #fff;
		border-radius: 16px;
		padding: 28px;
		width: 440px;
		max-width: 92vw;
		box-shadow: 0 24px 64px rgba(0, 0, 0, 0.25);
	}

	.popup-header {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-bottom: 20px;
	}

	.icon {
		font-size: 24px;
	}

	h3 {
		margin: 0;
		font-size: 17px;
		font-weight: 600;
		color: #1e293b;
	}

	.detail {
		background: #f8fafc;
		border-radius: 10px;
		padding: 14px 16px;
		margin-bottom: 22px;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.row {
		display: flex;
		gap: 12px;
		align-items: baseline;
	}

	.label {
		font-size: 12px;
		font-weight: 600;
		color: #64748b;
		min-width: 72px;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.value {
		font-size: 13px;
		color: #1e293b;
		word-break: break-all;
	}

	.mono {
		font-family: monospace;
		font-size: 12px;
	}

	.actions {
		display: flex;
		gap: 8px;
		justify-content: flex-end;
	}

	.btn {
		padding: 9px 18px;
		border: none;
		border-radius: 8px;
		font-size: 13px;
		font-weight: 500;
		cursor: pointer;
		transition: background 0.15s;
	}

	.deny {
		background: #fee2e2;
		color: #b91c1c;
	}

	.deny:hover {
		background: #fecaca;
	}

	.once {
		background: #fef3c7;
		color: #92400e;
	}

	.once:hover {
		background: #fde68a;
	}

	.permanent {
		background: #2563eb;
		color: #fff;
	}

	.permanent:hover {
		background: #1d4ed8;
	}
</style>

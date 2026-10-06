const state = {
  lastManifest: null,
  lastManifestCommand: "",
  aliases: null,
  proposals: [],
};

function $(id) {
  return document.getElementById(id);
}

function currentConfig() {
  return {
    rpcUrl: $("rpc-url").value.trim(),
    vectorProgramId: $("vector-program-id").value.trim(),
    vaultProgramId: $("vault-program-id").value.trim(),
    feePayerFile: $("fee-payer-file").value.trim(),
    aliasesFile: $("aliases-file").value.trim(),
    vaultAlias: $("vault-alias").value.trim(),
    vaultSeed: $("vault-seed").value.trim(),
    vaultPda: $("vault-pda").value.trim(),
    memberAlias: $("member-alias").value.trim(),
    memberPda: $("member-pda").value.trim(),
  };
}

function baseArgs(includeFeePayer = false) {
  const config = currentConfig();
  const args = [
    "--rpc-url", config.rpcUrl,
    "--vector-program-id", config.vectorProgramId,
    "--vault-program-id", config.vaultProgramId,
  ];
  if (includeFeePayer && config.feePayerFile) {
    args.push("--fee-payer-file", config.feePayerFile);
  }
  if (config.aliasesFile) {
    args.push("--aliases-file", config.aliasesFile);
  }
  if (config.vaultAlias) {
    args.push("--vault-alias", config.vaultAlias);
  } else if (config.vaultPda) {
    args.push("--vault", config.vaultPda);
  } else if (config.vaultSeed) {
    args.push("--vault-seed", config.vaultSeed);
  }
  return args;
}

async function apiGet(path) {
  const response = await fetch(path);
  return response.json();
}

async function apiPost(path, payload) {
  const response = await fetch(path, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const data = await response.json();
  if (!response.ok || !data.ok) {
    throw new Error(data.error || data.stderr || "request failed");
  }
  return data;
}

function setStatus(id, text, tone = "muted") {
  const node = $(id);
  node.textContent = text;
  node.className = `status-pill ${tone}`;
}

function setCode(id, value) {
  const node = $(id);
  node.textContent = typeof value === "string" ? value : JSON.stringify(value, null, 2);
  node.classList.remove("muted");
}

function boolWord(value) {
  return value ? "yes" : "no";
}

function manifestCommand(manifest) {
  if (!manifest) {
    return "";
  }
  return [manifest.script, ...manifest.args]
    .map((part) => /[^A-Za-z0-9_./:=,-]/.test(part) ? `'${part.replace(/'/g, `'\\''`)}'` : part)
    .join(" ");
}

function renderVault(vaultJson) {
  setCode("vault-json", vaultJson || "No vault loaded");
}

function renderProposals(payload) {
  state.proposals = payload.proposals || [];
  $("proposal-meta").textContent = `matches=${payload.matches} current_slot=${payload.current_slot} config=${payload.current_config_version}`;
  const tbody = $("proposal-rows");
  tbody.innerHTML = "";

  if (!state.proposals.length) {
    tbody.innerHTML = '<tr><td colspan="7" class="muted">No proposals matched the current filters</td></tr>';
    return;
  }

  for (const proposal of state.proposals) {
    const tr = document.createElement("tr");
    const summary = proposal.summary || "";
    const memberStatus = proposal.member_status || {};
    const approvalText = `${proposal.approval_count}/${proposal.threshold}`;
    const actions = [];
    actions.push(`<button type="button" data-action="inspect" data-id="${proposal.proposal_id}">Inspect</button>`);
    if (proposal.queue_status === "needs-approval") {
      actions.push(`<button type="button" data-action="approve" data-id="${proposal.proposal_id}">Prepare Approve</button>`);
    }
    if (proposal.queue_status === "executable") {
      actions.push(`<button type="button" data-action="execute" data-id="${proposal.proposal_id}">Prepare Execute</button>`);
    }
    if (proposal.queue_status === "closable") {
      actions.push(`<button type="button" data-action="close" data-id="${proposal.proposal_id}">Prepare Close</button>`);
    }
    if (memberStatus.member_pda && proposal.state === "Active") {
      actions.push(`<button type="button" data-action="cancel" data-id="${proposal.proposal_id}">Prepare Cancel</button>`);
      if (memberStatus.has_approved) {
        actions.push(`<button type="button" data-action="revoke" data-id="${proposal.proposal_id}">Prepare Revoke</button>`);
      }
    }

    tr.innerHTML = `
      <td>${proposal.proposal_id}</td>
      <td>${proposal.state_label}</td>
      <td>${proposal.queue_status}</td>
      <td>${proposal.action_kind_label}</td>
      <td>${approvalText}</td>
      <td>
        <div>${summary}</div>
        ${memberStatus.member_pda ? `<div class="subtle">mine=${boolWord(memberStatus.is_active)} approved=${boolWord(memberStatus.has_approved)} can_approve=${boolWord(memberStatus.can_approve)}</div>` : ""}
      </td>
      <td class="action-cell">${actions.join("")}</td>
    `;
    tbody.appendChild(tr);
  }
}

async function loadAliases() {
  const path = $("aliases-file").value.trim();
  if (!path) {
    throw new Error("aliases file is required");
  }
  const data = await apiGet(`/api/aliases?path=${encodeURIComponent(path)}`);
  state.aliases = data;

  const vaultSelect = $("vault-alias");
  const memberSelect = $("member-alias");
  vaultSelect.innerHTML = '<option value="">Select vault alias</option>';
  memberSelect.innerHTML = '<option value="">Select member alias</option>';

  for (const alias of data.vault_aliases || []) {
    const option = document.createElement("option");
    option.value = alias;
    option.textContent = alias;
    vaultSelect.appendChild(option);
  }
  for (const alias of data.member_aliases || []) {
    const option = document.createElement("option");
    option.value = alias;
    option.textContent = alias;
    memberSelect.appendChild(option);
  }
  setStatus("alias-status", `loaded ${data.vault_aliases.length} vaults / ${data.member_aliases.length} members`, "ok");
}

async function refreshVault() {
  const args = [...baseArgs(false), "--mode", "show-vault", "--json"];
  const data = await apiPost("/api/ops", { args, parse_json: true });
  renderVault(data.json);
}

async function refreshProposals() {
  const config = currentConfig();
  const args = [
    ...baseArgs(false),
    "--mode", "list-proposals",
    "--state", $("state-filter").value,
    "--queue", $("queue-filter").value,
    "--action-kind", $("action-kind-filter").value,
    "--json",
  ];
  if (config.memberAlias) {
    args.push("--member-alias", config.memberAlias);
  } else if (config.memberPda) {
    args.push("--member", config.memberPda);
  }
  const data = await apiPost("/api/ops", { args, parse_json: true });
  renderProposals(data.json);
}

async function inspectProposal(proposalId) {
  const args = [...baseArgs(false), "--mode", "show-proposal", "--proposal-id", String(proposalId), "--json"];
  const data = await apiPost("/api/ops", { args, parse_json: true });
  setCode("proposal-json", data.json);
}

async function prepareManifest(action, proposalId = null) {
  const config = currentConfig();
  const args = [...baseArgs(true), "--mode", action];

  if (["approve", "revoke", "cancel", "accept-pending-member"].includes(action)) {
    if (config.memberAlias) {
      args.push("--member-alias", config.memberAlias);
    } else if (config.memberPda) {
      args.push("--member", config.memberPda);
    } else {
      throw new Error(`${action} requires a member alias or member PDA`);
    }
  }

  if (proposalId !== null) {
    args.push("--proposal-id", String(proposalId));
  }

  args.push("--manifest-only");

  const data = await apiPost("/api/ops", { args, parse_json: true });
  state.lastManifest = data.json;
  state.lastManifestCommand = manifestCommand(data.json);
  setCode("manifest-json", data.json);
}

async function checkHealth() {
  const data = await apiGet("/api/health");
  setStatus("health-status", data.ok ? "Dashboard ready" : "Health failed", data.ok ? "ok" : "error");
}

async function copyText(value) {
  if (!value) {
    return;
  }
  await navigator.clipboard.writeText(value);
}

document.addEventListener("click", async (event) => {
  const button = event.target.closest("button[data-action]");
  if (!button) {
    return;
  }
  try {
    const proposalId = Number(button.dataset.id);
    const action = button.dataset.action;
    if (action === "inspect") {
      await inspectProposal(proposalId);
      return;
    }
    await prepareManifest(action, proposalId);
  } catch (error) {
    setCode("manifest-json", { error: String(error) });
  }
});

$("health-button").addEventListener("click", async () => {
  try {
    await checkHealth();
  } catch (error) {
    setStatus("health-status", String(error), "error");
  }
});

$("load-aliases").addEventListener("click", async () => {
  try {
    await loadAliases();
  } catch (error) {
    setStatus("alias-status", String(error), "error");
  }
});

$("refresh-vault").addEventListener("click", async () => {
  try {
    await refreshVault();
  } catch (error) {
    setCode("vault-json", { error: String(error) });
  }
});

$("refresh-proposals").addEventListener("click", async () => {
  try {
    await refreshProposals();
  } catch (error) {
    setCode("proposal-json", { error: String(error) });
  }
});

$("prepare-accept").addEventListener("click", async () => {
  try {
    await prepareManifest("accept-pending-member");
  } catch (error) {
    setCode("manifest-json", { error: String(error) });
  }
});

$("copy-manifest-command").addEventListener("click", async () => {
  await copyText(state.lastManifestCommand);
});

$("copy-manifest-json").addEventListener("click", async () => {
  if (state.lastManifest) {
    await copyText(JSON.stringify(state.lastManifest, null, 2));
  }
});

checkHealth().catch(() => {});

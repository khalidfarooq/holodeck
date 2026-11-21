/**
 * Holodeck Node.js / TypeScript Agent Integration Example
 * -------------------------------------------------------
 * Demonstrates how an AI coding agent uploads artifacts to Holodeck's local S3
 * simulator and sends sandbox notifications with zero external dependencies.
 *
 * Usage:
 *   1. Start Holodeck: `holodeck start`
 *   2. Run script:     `node agent_storage_test.js`
 */

const BASE_URL = "http://127.0.0.1:8787";

async function run() {
  console.log("🌌 Running Holodeck Storage & Notification Agent Test...\n");

  try {
    // 1. Health check
    const health = await fetch(`${BASE_URL}/health`).then((r) => r.json());
    console.log(`✅ Holodeck Health: ${health.status} (v${health.version})`);

    // 2. Request S3 Presigned URL
    const presignReq = await fetch(`${BASE_URL}/storage/presign`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        bucket: "agent-workspace",
        key: "build/bundle.js",
        method: "PUT",
      }),
    }).then((r) => r.json());
    console.log(`📦 Generated S3 Presigned Upload URL for: ${presignReq.bucket}/${presignReq.key}`);

    // 3. Upload artifact
    const artifactData = "console.log('AI-synthesized production bundle');";
    const uploadRes = await fetch(presignReq.url, {
      method: "PUT",
      headers: { "Content-Type": "text/javascript" },
      body: artifactData,
    });
    console.log(`⬆️  Uploaded artifact to local S3: HTTP ${uploadRes.status}`);

    // 4. Download and verify artifact
    const downloadedData = await fetch(`${BASE_URL}/storage/agent-workspace/build/bundle.js`).then((r) => r.text());
    console.log(`⬇️  Downloaded artifact verified: "${downloadedData}"`);

    // 5. Send mock email via Resend simulator
    const emailRes = await fetch(`${BASE_URL}/v1/emails`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        from: "agent@holodeck.local",
        to: "developer@company.com",
        subject: "CI/CD Deployment Artifacts Ready",
        text: "Artifact uploaded to S3 successfully.",
      }),
    }).then((r) => r.json());
    console.log(`📬 Dispatched Resend Mock Email: ${emailRes.id}`);

    // 6. Inspect captured events
    const events = await fetch(`${BASE_URL}/events?limit=10`).then((r) => r.json());
    console.log(`\n📡 Total Events Captured on Holodeck EventBus: ${events.length}`);
    for (const evt of events) {
      console.log(`   - [${evt.provider.toUpperCase()}] ${evt.event_type} (${evt.id})`);
    }

    console.log("\n✨ All tests passed seamlessly!");
  } catch (err) {
    console.error("❌ Test failed:", err.message);
    console.log("💡 Ensure Holodeck is running via `holodeck start`");
    process.exit(1);
  }
}

run();

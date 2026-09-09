package org.atomicprivatecontinuum.harness;

import android.app.Activity;
import android.os.Bundle;
import android.util.Log;
import android.view.Gravity;
import android.widget.LinearLayout;
import android.widget.TextView;

public final class MainActivity extends Activity {
    private static final String TAG = "APC-HARNESS";

    private TextView status;
    private boolean gateBeforeOnStart;
    private String probeResult = "not requested";

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        gateBeforeOnStart = NativeBridge.isForeground();
        runRequestedProbe();

        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setGravity(Gravity.CENTER_VERTICAL);
        int padding = Math.round(24 * getResources().getDisplayMetrics().density);
        root.setPadding(padding, padding, padding, padding);

        TextView title = new TextView(this);
        title.setText("A.P.C. Android harness");
        title.setTextSize(22);
        root.addView(title);

        status = new TextView(this);
        status.setTextSize(15);
        int topMargin = Math.round(16 * getResources().getDisplayMetrics().density);
        LinearLayout.LayoutParams statusParams = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
        );
        statusParams.topMargin = topMargin;
        root.addView(status, statusParams);

        setContentView(root);
        render("onCreate");
    }

    @Override
    protected void onStart() {
        super.onStart();
        NativeBridge.enterForeground();
        render("onStart → foreground");
    }

    @Override
    protected void onStop() {
        NativeBridge.enterBackground();
        render("onStop → background");
        super.onStop();
    }

    private void runRequestedProbe() {
        String command = getIntent().getStringExtra("apc_probe");
        if (command == null) {
            return;
        }

        String filesDir = getFilesDir().getAbsolutePath();
        switch (command) {
            case "stage":
                probeResult = NativeBridge.stageRecoveryProbe(filesDir);
                break;
            case "verify":
                probeResult = NativeBridge.verifyRecoveryProbe(filesDir);
                break;
            default:
                probeResult = "FAIL unknown apc_probe command: " + command;
                break;
        }
    }

    private void render(String event) {
        if (status == null) {
            return;
        }

        String text = "bridge: " + NativeBridge.version()
                + "\nprocess gate before onStart: " + gateBeforeOnStart
                + "\nprocess gate now: " + NativeBridge.isForeground()
                + "\nrecovery probe: " + probeResult
                + "\nlast lifecycle event: " + event;
        status.setText(text);
        Log.i(TAG, text.replace('\n', '|'));
    }
}

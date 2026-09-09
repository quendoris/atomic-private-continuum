package org.atomicprivatecontinuum.harness;

public final class NativeBridge {
    static {
        System.loadLibrary("apc_android_bridge");
    }

    private NativeBridge() {}

    private static native String nativeVersion();
    private static native void nativeEnterForeground();
    private static native void nativeEnterBackground();
    private static native boolean nativeIsForeground();
    private static native String nativeStageRecoveryProbe(String filesDir);
    private static native String nativeVerifyRecoveryProbe(String filesDir);

    public static String version() {
        return nativeVersion();
    }

    public static void enterForeground() {
        nativeEnterForeground();
    }

    public static void enterBackground() {
        nativeEnterBackground();
    }

    public static boolean isForeground() {
        return nativeIsForeground();
    }

    public static String stageRecoveryProbe(String filesDir) {
        return nativeStageRecoveryProbe(filesDir);
    }

    public static String verifyRecoveryProbe(String filesDir) {
        return nativeVerifyRecoveryProbe(filesDir);
    }
}

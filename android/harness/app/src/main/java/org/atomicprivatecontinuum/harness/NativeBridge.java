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
}

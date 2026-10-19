package probe;
import dependency.Attached;
import dependency.BinaryJava;
import dependency.BinaryKotlin;
public class Main {
    java.util.HashMap<String, String> map; // JDK_SOURCE
    Attached attached; // ATTACHED_SOURCE
    BinaryJava binary; // BINARY_JAVA
    BinaryKotlin kotlin; // BINARY_KOTLIN
    String member = new BinaryJava().message(); // BINARY_MEMBER
    int broken = "wrong"; // DIAGNOSTIC
}

package probe
import dependency.AttachedKotlin
import dependency.BinaryJava
import dependency.BinaryKotlin
val map: java.util.HashMap<String, String>? = null // JDK_SOURCE
val attached: AttachedKotlin? = null // ATTACHED_SOURCE
val binary: BinaryJava? = null // BINARY_JAVA
val kotlin: BinaryKotlin? = null // BINARY_KOTLIN
val member = BinaryJava().message() // BINARY_MEMBER
val broken: Int = "wrong" // DIAGNOSTIC

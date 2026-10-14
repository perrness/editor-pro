package probe

fun main() {
    val message = greet("reader") // PROBE_DEFINITION
    println(message.length) // PROBE_COMPLETION
    val broken: Int = "wrong" // PROBE_DIAGNOSTIC
    println(broken)
}

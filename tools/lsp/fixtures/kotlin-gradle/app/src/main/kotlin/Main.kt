package probe

fun main() {
    val message = Greeter.greet("reader") // DEFINITION
    println(message.length) // COMPLETION
    val broken: Int = "wrong" // DIAGNOSTIC
    println(broken)
    println("😀 é界")
}

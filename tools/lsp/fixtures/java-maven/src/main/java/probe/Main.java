package probe;

public class Main {
    public static void main(String[] args) {
        String message = Greeter.greet("reader"); // DEFINITION
        System.out.println(message.length()); // COMPLETION
        int broken = "wrong"; // DIAGNOSTIC
        System.out.println(broken);
        System.out.println("😀 é界");
    }
}

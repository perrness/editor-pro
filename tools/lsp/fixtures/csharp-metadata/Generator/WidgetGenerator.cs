using Microsoft.CodeAnalysis;

[Generator]
public class WidgetGenerator : ISourceGenerator {
    public void Initialize(GeneratorInitializationContext context) { }

    public void Execute(GeneratorExecutionContext context) {
        context.AddSource("GeneratedWidget.g.cs", """
            // Generated source with Unicode: 😀 é界
            public class GeneratedWidget {
                public static string Message() => "generated";
            }
            """);
    }
}

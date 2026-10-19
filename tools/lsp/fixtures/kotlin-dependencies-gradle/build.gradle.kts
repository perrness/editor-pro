plugins { kotlin("jvm") version "2.3.0" }
repositories {
    maven { url = uri("repository") }
    mavenCentral()
}
dependencies { implementation("dev.editorpro:dependency-fixture:0.0.27") }
java { toolchain { languageVersion.set(JavaLanguageVersion.of(21)) } }

package com.triangulararb.dashboard.arch;

import com.tngtech.archunit.core.domain.JavaClasses;
import com.tngtech.archunit.core.importer.ClassFileImporter;
import com.tngtech.archunit.core.importer.ImportOption;
import com.tngtech.archunit.lang.ArchRule;
import org.junit.jupiter.api.Test;

import static com.tngtech.archunit.lang.syntax.ArchRuleDefinition.classes;
import static com.tngtech.archunit.library.Architectures.layeredArchitecture;

class HexagonalArchTest {

    private static final JavaClasses imported = new ClassFileImporter()
            .withImportOption(ImportOption.Predefined.DO_NOT_INCLUDE_TESTS)
            .importPackages("com.triangulararb.dashboard");

    @Test
    void domainShouldNotDependOnInfrastructure() {

        ArchRule rule = classes()
                .that().resideInAPackage("..domain..")
                .should().onlyDependOnClassesThat()
                .resideInAnyPackage("..domain..", "java..");

        rule.check(imported);
    }

    @Test
    void applicationShouldOnlyDependOnDomain() {
        ArchRule rule = classes()
                .that().resideInAPackage("..application..")
                .should().onlyDependOnClassesThat()
                .resideInAnyPackage("..domain..", "..application..", "java..", "org.springframework..");

        rule.check(imported);
    }

    @Test
    void infrastructureShouldNotBeCalledByUpperLayers() {
        ArchRule rule = classes()
                .that().resideInAPackage("..domain..")
                .should().onlyAccessClassesThat()
                .resideInAnyPackage("..domain..", "java..");

        ArchRule application = classes()
                .that().resideInAPackage("..application..")
                .should().onlyAccessClassesThat()
                .resideInAnyPackage("..domain..", "..application..", "java..", "org.springframework..");

        rule.check(imported);
        application.check(imported);
    }

    @Test
    void infrastructureShouldImplementDomainPorts() {
        ArchRule rule = classes()
                .that().resideInAPackage("..infrastructure..")
                .should().onlyDependOnClassesThat()
                .resideInAnyPackage("..infrastructure..", "..domain..", "..application..",
                        "java..", "org.springframework..", "com.fasterxml..",
                        "org.slf4j..", "jakarta..");

        rule.check(imported);
    }

    @Test
    void layeredArchitectureShouldBeRespected() {

        var rule = layeredArchitecture()
                .consideringAllDependencies()
                .layer("Domain").definedBy("..domain..")
                .layer("Application").definedBy("..application..")
                .layer("Infrastructure").definedBy("..infrastructure..")
                .whereLayer("Domain").mayOnlyBeAccessedByLayers("Application", "Infrastructure")
                .whereLayer("Application").mayOnlyBeAccessedByLayers("Infrastructure")
                .whereLayer("Infrastructure").mayNotBeAccessedByAnyLayer();

        rule.check(imported);
    }
}

#!/usr/bin/env php
<?php
/**
 * Generate schema v1 catalogs using the current runtime (PHP 7.0+).
 *   php scripts/generate_php_stubs.php --output=php_8.5.json
 * Docker (the selected image/extensions define the resulting inventory):
 *   docker run --rm -v "$PWD:/work" -w /work php:8.5-cli php scripts/generate_php_stubs.php --output=php_8.5.json
 * Mount a host output directory or copy the result to ~/.nozhik/languages/php/versions/.
 * No PHP function or method discovered by Reflection is executed.
 */
function typeText($type) {
    if ($type === null) { return 'mixed'; }
    // PHP 7.0 ReflectionType has no getName(); string conversion is portable.
    $text = (string) $type;
    if (method_exists($type, 'allowsNull') && $type->allowsNull()
        && $text !== 'mixed' && $text !== 'null' && strpos($text, '?') !== 0
        && strpos($text, 'null') === false) {
        $text = strpos($text, '|') === false ? '?' . $text : $text . '|null';
    }
    return $text;
}
function valueText($value) {
    if (is_resource($value)) { return 'resource'; }
    if (is_object($value)) { return 'new ' . get_class($value) . '()'; }
    // var_export produces valid escaped PHP literals, including binary strings.
    return var_export($value, true);
}
function callableSymbol(ReflectionFunctionAbstract $reflection, $kind) {
    $parameters = array();
    foreach ($reflection->getParameters() as $p) {
        $parameter = array('name' => $p->getName(), 'type' => typeText($p->getType()),
            'optional' => $p->isOptional(), 'variadic' => $p->isVariadic(),
            'by_reference' => $p->isPassedByReference());
        if ($p->isDefaultValueAvailable()) {
            $parameter['default'] = $p->isDefaultValueConstant()
                ? $p->getDefaultValueConstantName() : valueText($p->getDefaultValue());
        }
        $parameters[] = $parameter;
    }
    $return = $reflection->getReturnType();
    if ($return === null && method_exists($reflection, 'getTentativeReturnType')) {
        $return = $reflection->getTentativeReturnType();
    }
    $name = $reflection->getName();
    $symbol = array('kind' => $kind, 'name' => $name, 'parameters' => $parameters,
        'return_type' => typeText($return), 'documentation' => $reflection->getDocComment() ?: '',
        'package' => $reflection->getExtensionName() ?: 'Core', 'snippet' => $name . '(${0})');
    if ($reflection instanceof ReflectionMethod) { $symbol['is_static'] = $reflection->isStatic(); }
    return $symbol;
}
function constantSymbol($name, $value, $package, $type = null) {
    $types = array('integer' => 'int', 'double' => 'float', 'boolean' => 'bool', 'NULL' => 'null');
    $native = gettype($value);
    return array('kind' => 'constant', 'name' => $name,
        'type' => $type === null ? (isset($types[$native]) ? $types[$native] : $native) : typeText($type),
        'value' => valueText($value), 'documentation' => '', 'package' => $package);
}
try {
    $options = getopt('', array('output:', 'help'));
    if (isset($options['help'])) {
        fwrite(STDOUT, "Usage: php scripts/generate_php_stubs.php --output=php_<major.minor>.json\n");
        exit(0);
    }
    if (!isset($options['output']) || !is_string($options['output']) || $options['output'] === '') {
        throw new RuntimeException('Required option: --output=<path>');
    }
    $entities = array();
    $functions = get_defined_functions();
    foreach ($functions['internal'] as $name) {
        $entities[] = callableSymbol(new ReflectionFunction($name), 'function');
    }
    $seenClasses = array();
    foreach (array_merge(get_declared_classes(), get_declared_interfaces()) as $name) {
        $class = new ReflectionClass($name);
        if (!$class->isInternal()) { continue; }
        // Aliases may be returned with the canonical name; keep only one entity.
        $canonical = $class->getName();
        if (isset($seenClasses[strtolower($canonical)])) { continue; }
        $seenClasses[strtolower($canonical)] = true;
        $package = $class->getExtensionName() ?: 'Core';
        $methods = array();
        foreach ($class->getMethods(ReflectionMethod::IS_PUBLIC) as $method) {
            $methods[] = callableSymbol($method, 'method');
        }
        $constants = array();
        if (method_exists($class, 'getReflectionConstants')) {
            foreach ($class->getReflectionConstants() as $constant) {
                if (!$constant->isPublic()) { continue; }
                $type = method_exists($constant, 'getType') ? $constant->getType() : null;
                $constants[] = constantSymbol($constant->getName(), $constant->getValue(), $package, $type);
            }
        } else {
            // Class constant visibility was introduced in 7.1; PHP 7.0 constants are public.
            foreach ($class->getConstants() as $key => $value) {
                $constants[] = constantSymbol($key, $value, $package);
            }
        }
        $entities[] = array('kind' => $class->isInterface() ? 'interface' : 'class',
            'name' => $canonical, 'documentation' => $class->getDocComment() ?: '',
            'package' => $package, 'methods' => $methods, 'constants' => $constants);
    }
    // Includes Core and every loaded extension; excludes user-defined constants.
    foreach (get_defined_constants(true) as $extension => $constants) {
        if ($extension === 'user') { continue; }
        foreach ($constants as $name => $value) {
            $entities[] = constantSymbol($name, $value, $extension);
        }
    }
    $keywords = '__halt_compiler die abstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile eval exit extends final finally for foreach function global goto if implements include include_once instanceof insteadof interface isset list namespace new or print private protected public require require_once return static switch throw trait try unset use var while xor yield';
    if (PHP_VERSION_ID >= 70100) { $keywords .= ' iterable void'; }
    if (PHP_VERSION_ID >= 70400) { $keywords .= ' fn'; }
    if (PHP_VERSION_ID >= 80000) { $keywords .= ' match mixed'; }
    if (PHP_VERSION_ID >= 80100) { $keywords .= ' readonly enum never'; }
    $keywords = explode(' ', $keywords);
    $keywords[] = 'yield from';
    foreach ($keywords as $keyword) {
        $entities[] = array('kind' => 'keyword', 'name' => $keyword, 'snippet' => $keyword);
    }
    // Magic constants are context-dependent and are absent from get_defined_constants().
    $magic = array('__CLASS__', '__DIR__', '__FILE__', '__FUNCTION__', '__LINE__', '__METHOD__', '__NAMESPACE__', '__TRAIT__');
    if (PHP_VERSION_ID >= 80400) { $magic[] = '__PROPERTY__'; }
    foreach ($magic as $name) {
        $entities[] = array('kind' => 'constant', 'name' => $name,
            'type' => $name === '__LINE__' ? 'int' : 'string', 'value' => null,
            'documentation' => 'Compile-time constant; value depends on the source context.', 'package' => 'language');
    }
    usort($entities, function ($a, $b) { return strcmp($a['kind'] . ':' . $a['name'], $b['kind'] . ':' . $b['name']); });
    $extensions = get_loaded_extensions();
    sort($extensions);
    $document = array('schema_version' => 1, 'version' => PHP_MAJOR_VERSION . '.' . PHP_MINOR_VERSION,
        'extensions' => $extensions, 'entities' => $entities);
    // Substitute invalid UTF-8 bytes in unusual platform constants on PHP >=7.2.
    $flags = JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE;
    if (defined('JSON_INVALID_UTF8_SUBSTITUTE')) { $flags |= JSON_INVALID_UTF8_SUBSTITUTE; }
    $json = json_encode($document, $flags);
    if ($json === false) { throw new RuntimeException(json_last_error_msg()); }
    $output = $options['output'];
    if (is_link($output)) { throw new RuntimeException('Refusing to replace a symbolic link'); }
    $temporary = tempnam(dirname($output), '.php-stubs-');
    if ($temporary === false) { throw new RuntimeException('Cannot create output file'); }
    try {
        if (file_put_contents($temporary, $json . "\n") === false || !rename($temporary, $output)) {
            throw new RuntimeException('Cannot write output file');
        }
    } finally {
        if (is_file($temporary)) { unlink($temporary); }
    }
    fwrite(STDERR, 'Wrote ' . count($entities) . ' entities for PHP ' . $document['version'] . ' to ' . $output . "\n");
} catch (Throwable $error) {
    fwrite(STDERR, 'PHP stubs: ' . $error->getMessage() . "\n");
    exit(1);
}
